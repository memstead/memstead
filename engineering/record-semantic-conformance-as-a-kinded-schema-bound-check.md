---
type: decision
created_date: 2026-08-26T12:24:20Z
last_modified: 2026-08-26T12:24:20Z
status: accepted
decided_on: 2026-08-26
deciders: operator
scope: subsystem
tags: checks, schema, conformance, trust
---

# Record semantic conformance as a kinded, schema-bound check

## Decision
We will extend the check-record ledger with a closed check-kind vocabulary (`verification`, the default and today's behaviour, plus `conformance` for semantic schema conformance) and an engine-stamped `schema_ref`, and derive check state per (entity, kind) instead of per entity. A `conformance` verdict goes stale when the entity's content hash moves OR the mem's schema pin moves; judgments are produced by the agent loop (the tidy skill, role `checker`) against the pinned schema's `write_rules` and `writing_guidance` prose. The engine never judges: it records acts and derives state, exactly as [[engine--check-records-and-review-marks]] already does for the single-stream case.

## Context
The [[engine--runtime-validator]] enforces the structural half of every schema at the write gate: sections, fields, enums, relationship vocabulary. The semantic half, the per-type prose (`write_rules`, `writing_guidance`), is shown to the writing agent once via `memstead_schema` and never verified afterwards: an entity can pass the validator and still miss what its type exists for (a decision without rejected alternatives), and editing the prose invalidates nothing. The 2026-08 competitor research found the market converging on structural enforcement (the direct competitor's roadmap moves toward write-path validation) while zby/commonplace demonstrated, in production, that type-spec prose can serve as an executable LLM-judged review criterion whose freshness is computable when the verdict is bound to content hash, criterion version, and judging model. Memstead's check ledger already records verdicts append-only with role provenance and hash-based staleness; what it lacks is exactly a kind discriminator (all checks form one stream, so a later check of another sort supersedes a conformance verdict) and a schema binding (a re-pinned schema leaves stale verdicts looking fresh).

## Consequences
- The semantic half of a schema becomes a checkable contract with four honest per-entity states (never checked / ok / failed / stale) served through the existing health and provenance surfaces.
- Schema-prose edits become consequential: a new pin flips all conformance verdicts of that mem to stale, which is the intended forcing function.
- The `memstead_check` parameter addition propagates to every surface consumer (tool roster tests, generated docs, CLI parity) in the same change.
- Ongoing LLM cost lands only in maintenance runs (tidy), never in the write or query path; an operator can decline to run judgments indefinitely and the system reports honest `never_checked` counts instead of lying.
- Verdicts are advisory, never gates: a `failed` conformance check blocks no write and no read, it only feeds worklists. LLM judgments are non-deterministic; the record therefore names the judging model in its method note so a verdict is never mistaken for a deterministic fact.
- Old ledger lines stay valid: a record without a kind reads as `verification`, so the extension is backward compatible with existing workspaces.

## Relationships
- **REFERENCES**: [[engine:runtime-validator]]
- **REFERENCES**: [[engine:check-records-and-review-marks]]

## Options

- Do nothing, prose stays a write-time instruction: rejected, it leaves the highest-value half of every schema unenforced exactly while the market catches up on the structural half.
- Skill-only judging without recording (tidy critiques ad hoc): rejected, verdicts would decay invisibly, nobody could say what still holds after an edit or a re-pin, and the friction of re-judging everything every run is unbounded.
- A dedicated MCP conformance tool: rejected by the standing tool-surface policy, extending `memstead_check` parameters covers it without a new tool.
- An LLM gate at write time: rejected, it would put a non-deterministic model call in the mutation path, violating the engine's determinism posture and the agent-loop-is-the-runtime operating model.
- Kinded, schema-bound check records judged by the agent loop: chosen.

## Notes

Granularity starts at one verdict per (entity, kind): per-section or per-rule verdicts are a possible later refinement, as are per-model verdict partitions (Commonplace partitions by judging model; here the model initially rides in the free-text method note). The kind vocabulary is engine-closed, matching the closed verdict vocabulary, so health aggregation stays well-defined; opening it is a separate decision if a third kind ever earns its place.
