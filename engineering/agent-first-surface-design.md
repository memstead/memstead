---
type: principle
created_date: 2026-07-13T16:43:02Z
last_modified: 2026-07-13T16:43:59Z
authority: established
universality: domain-wide
tags: agent-first, mcp, surface-design, ergonomics, primary-consumer, wire-surface, engine
---

# Agent-first surface design

## Statement
The engine's machine-facing surfaces are designed from the primary consumer's perspective first, and that primary consumer is the LLM agent. Formats, tool shapes, parameter names, defaults, and error payloads are evaluated for how an agent reads them, queries with them, and recovers from them — before any human-readability consideration. Human-facing projections (the macOS app, rendered documentation, mem summaries) are a deliberately separate layer, designed separately for human ergonomics over the same substrate.

## Scope
Governs every surface an agent consumes directly: the [[engine--mcp-tool-surface]] (full and lean), the [[engine--schema-describe-projection-surface]] and the relationship vocabulary, the [[engine--typed-error-and-warning-envelope]], and the read/search/entity projections — plus the CLI text and the UniFFI reads the macOS app's in-app agent drives. It does NOT govern the human projection layer (the macOS app's own UI chrome, the docs-site rendering, prose summaries), which optimises for human reading on the same files. Two consumer profiles, one shared markdown-in-git substrate.

## Relationships
- **REFERENCES**: [[engine:mcp-tool-surface]]
- **REFERENCES**: [[engine:schema-describe-projection-surface]]
- **REFERENCES**: [[engine:typed-error-and-warning-envelope]]
- **REFERENCES**: [[mcp-tool-surface-stays-small]]
- **GOVERNS**: [[engine:mcp-tool-surface]]
- **GOVERNS**: [[engine:schema-describe-projection-surface]]
- **GOVERNS**: [[engine:typed-error-and-warning-envelope]]
- **GENERALIZES**: [[mcp-tool-surface-stays-small]]

## Justification

Stated as the project's foundational design stance (`AGENTS.md`: "built for LLM agents as the primary consumer... evaluate them from the agent's perspective first"; `VISION.md`: "designed for LLM agents as the primary author and consumer... two layers, two consumer profiles, one shared substrate"). A surface optimised for humans first — verbose prose over structured fields, positional arguments, silent coercion of bad input — costs the agent the three things it is most scarce in: context budget, round-trips, and the ability to self-correct. Optimising the agent path first is therefore not a stylistic preference but the load-bearing product bet.

## Exceptions

The human projection layer is the deliberate exception: the macOS app UI, the docs-site, and prose mem summaries are designed for human ergonomics and may trade agent-optimality for readability. The split is intentional — this principle only claims the machine-facing surfaces, and cedes the human-facing ones to a separately-designed layer.

## Consequences

Concrete enactments across the codebase, each an instance of this one stance rather than an isolated choice:
- The [[engine--schema-describe-projection-surface]] defaults to the lite skeleton (a ~7 KB cold-start cost) instead of the ~52 KB full body, because the agent needs structural flags to plan a write, not prose, on the common path.
- Tool descriptions are fitted under the primary client's tool-description truncation ceiling (byte-guarded in the meta-test suite) so no contract text is silently cut off at the consumer, and cross-tool teaching is stated once in the server `instructions` rather than repeated per tool to hold that budget.
- Every mutation response is dual-channel: a typed `structured_content` envelope the agent branches on without parsing prose, mirrored by a human-renderable text channel — the shape modelled by [[engine--typed-error-and-warning-envelope]].
- Schema-conformance errors carry recovery payloads (declared lists, allowed enum values, nearest-match suggestions) so the agent fixes from the error rather than re-fetching the schema after every refusal.
- Parameter structs use `deny_unknown_fields` so a mis-scoped or stale-named call fails loud with the offending field named, rather than being silently ignored and returning wrong results an agent would trust.
- The tool surface stays small with additive optional fields on stable shapes, no action-discriminators, and no response-shape polymorphism — the more specific rule [[engineering--mcp-tool-surface-stays-small]] is a specialisation of this principle.
