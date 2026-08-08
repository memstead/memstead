---
type: decision
created_date: 2026-08-08T20:11:29Z
last_modified: 2026-08-08T20:11:29Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust plan 09)
scope: subsystem
tags: schema-format, exemplar, few-shot, validation, agent-surface
---

# Schemas teach by one engine-validated exemplar per type with no warn-and-carry mode

## Decision
A schema type may carry ONE canonical exemplar — a complete entity in the mem markdown shape (title, metadata, sections, relations with bare placeholder-slug targets) — and the exemplar is validated or it does not exist: the engine runs every exemplar through the REAL create validation stage (a `dry_run` create against an in-memory engine pinning the candidate schema — the same gates a real write runs, commit-free) at `memstead schema validate`, at folder install, and at mem-repo install/seal via the shared `validate_schema_package` gate. A non-conformant exemplar refuses with a typed error naming the type and defect code; there is deliberately NO warn-and-carry mode, because the entire value of an exemplar is the impossibility of drift. Serving is context-economical: `memstead_schema` at `verbosity: full` carries the exemplar with the type, the lite skeleton (fetched once per session per schema) is byte-unchanged, and the CLI's full-depth `memstead type` view renders it. Relation targets are bare placeholder slugs scoped to a virtual mem — rel-type legality and edge shape are validated, target existence never (an exemplar lives outside any mem). The old `examples:` list — which promised few-shot injection but was never validated nor served by any surface — is retired with the plan-06 posture: authoring loads refuse it with a typed pointer at `exemplar:`, sealed content keeps loading with the key dropped.

## Context
LLMs learn from examples far better than from rules — few-shot beats specification for every model class that consumes this engine — yet a schema taught exclusively by prose: section keys, write_rules, constraint declarations, with the agent refused into conformance ([[engine--runtime-validator]]). Hand-maintained examples in ordinary documentation die by drift; the 2026-08-07 verification found stale doc comments in the engine's own tree within weeks of writing, and the worked-example teaching package itself carried an `examples:` block whose comment claimed MCP injection that never existed — dead vocabulary lying about its effect, the exact class plan 06 retired. The write-rehearsal work supplied the natural validator: the create path's `dry_run` is the full validation stage, commit-free by contract, so exemplar validation IS the write validation — one gate, no second regime, per [[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]].

## Consequences
An agent authoring its first entity against an unfamiliar type reads a known-good shape instead of deriving one from prose — and what it reads is exactly what the validator accepts, forever, because CI and the install gate refuse divergence (built-in exemplars are gated by the same validator; the worked-example package models the practice under test). Costs accepted: exemplar authoring is real work per type (~44 built-in types across six schemas await their exemplars and new version directories under the append-only rule); the full-verbosity payload grows by one entity per type (bounded — a canonical entity, not a showcase); and third-party schemas may omit exemplars entirely (optional per type — only the reference schemas are held to completeness).

## Relationships
- **REFERENCES**: [[engine:runtime-validator]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

Better prose examples in write_rules (status quo) — rejected: unvalidated examples WILL drift; validation is the feature, not the formatting. Multiple exemplars per type (good/bad pairs, edge cases) — rejected for now: one canonical positive example captures most of the few-shot value, and negative examples are what refusal `details` payloads and the write rehearsal already provide interactively; revisit on friction-ledger evidence. Serving exemplars in the lite skeleton — rejected: lite is fetched every session by contract; growing it taxes every session for material only authoring sessions need.

## Notes


