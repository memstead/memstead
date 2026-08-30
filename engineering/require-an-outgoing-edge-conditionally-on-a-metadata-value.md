---
type: decision
created_date: 2026-08-23T00:46:11Z
last_modified: 2026-08-30T00:24:19Z
status: accepted
decided_on: 2026-08-21
deciders: operator (reasoning-substrate bundle, confirmed 2026-08-23), implementing agent
scope: subsystem
tags: schema, constraints, required-outgoing, conditional-edges, reasoning-substrate
---

# Require an outgoing edge conditionally on a metadata value

## Decision
We chose to let a `required_outgoing` block carry an optional `when_field` / `when_value` pair: the edge obligation applies only while the named metadata field of the entity holds the named enum value. The pair reuses the trigger vocabulary `requires_when` already speaks (one vocabulary for one idea), and the conditional block keeps every other semantic of the unconditional form: same cardinality vocabulary, same warn/block severity model, evaluated by the single shared function on create, update, the relate remove path, and the health sweep ([[engine--schema-definition-format]], [[engine--graph-health-report-surface]]). Every surface that names an unsatisfied block (the MISSING_REQUIRED_OUTGOING refusal payload, the write-time warning, the health finding, the memstead_schema response at both verbosity levels) carries the trigger, so the reader sees which value armed the obligation. The loader refuses one key without the other, an undeclared trigger field, a trigger field without enum_values, and a value outside the enum: stricter than requires_when, because an edge obligation armed by free text would never fire predictably.

## Context
First form of the reasoning-substrate wave (engine half of the accountable-reasoning program). The 2026-08-21 argument-schema experiment could force every inference to conclude something but could not express that an inference declaring a particular scheme value must carry the response edges that scheme demands. The capability is generic by construction: any per-value edge obligation (a task entering review requires a reviewer edge, an obligation marked critical requires an owner edge) uses the same form. The engine knows only that a field value implies an edge obligation; schemes and argumentation stay product vocabulary outside the engine.

## Consequences
- Each new key on the schema definition language is a format-generation event; this form is part of one declared release wave, and the release debt falls due with the wave's close-out plan, days after the last form lands.
- Until the wave's release tag existed, no built-in schema, example, or scaffold emitted the new keys; they lived only in test fixtures. The wave released as engine 0.10.0 and that clause has expired: the built-in `obligation` schema now declares `when_field` on a `required_outgoing` block, and the `memstead schema new` scaffold documents the key (verified 2026-08-30).
- Condition scope is deliberately closed: single field equals single enum value. Conjunctions, negations, comparisons, and section-presence conditions are out of scope.
- Cardinality stays at_least_one; per-question counts (one response edge per critical question) are not expressible and not claimed.
- Schemas without the new keys keep byte-identical engine behaviour and responses.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **REFERENCES**: [[engine:schema-definition-format]]
- **REFERENCES**: [[engine:graph-health-report-surface]]

## Options

- A new constraint kind (`requires_edges_when`): rejected, it would duplicate the cardinality and severity machinery the block already carries and give one concept two homes in the schema response.
- Condition on section presence instead of metadata value: rejected for this form, section presence is not a validated vocabulary; arguable later on its own evidence.
- Making scheme-specific critical questions an engine notion: rejected, schemes are product vocabulary.
- New key names instead of reusing when_field / when_value: rejected, the condition is the same concept requires_when expresses.

## Notes

Landed 2026-08-23 with reasoning-substrate plan 01; the keys shipped with the wave as engine **0.10.0** (public CHANGELOG `## [0.10.0] - 2026-08-23`, whose forward-compatibility note names `when_field` / `when_value`), so they no longer sit under [Unreleased] (verified 2026-08-30). Extends form 4 of the five-form constraint vocabulary rather than adding a sixth constraint kind, so the vocabulary's closed kind tag stays closed.
