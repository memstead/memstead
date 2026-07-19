---
type: decision
created_date: 2026-07-15T07:23:39Z
last_modified: 2026-07-15T07:34:24Z
status: accepted
decided_on: 2026-06-30
deciders: kara-maintainers
scope: org
tags: positioning, roadmap, strategy
---

# Backend-First v1 Positioning

## Decision
We chose to position Kāra's v1 as a backend-first, general-purpose systems language whose differentiators are the native LLVM backend, effect-driven concurrency, and the AI-first compiler interface — with a secondary 'data' bonus — rather than marketing it as a niche or data-only language. Graduated from brainstorm v64 (with v66 refining the general-purpose framing).

## Context
The brainstorm series debated Kāra's market identity across many versions. v64 settled the v1 story on the backend and concurrency strengths; v66 confirmed a general-purpose foundation with a data-processing 'quiet bonus' rather than a data-first pitch. Several positioning brainstorms (v63 LLM compiler query channel, v65 PGO/online-JIT) were graduated as durable direction.

## Consequences
- v1 investment concentrates on codegen quality, concurrency, and the compiler's agent-facing surface.
- PGO and online-JIT (v65) are recorded as future direction, not v1 scope.
- Framing drives the benchmark focus (Parallax HTTP fan-out) and the AI-first diagnostics work.

## Relationships
- **MOTIVATED_BY**: [[ai-first-compiler-interface]]

## Options

- Data-first / niche language — rejected: too narrow for the systems ambitions.
- General-purpose, backend-first with a data bonus — chosen (v64 + v66).

## Notes


