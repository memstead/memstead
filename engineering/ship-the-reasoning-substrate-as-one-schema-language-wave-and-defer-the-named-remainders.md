---
type: decision
created_date: 2026-08-23T03:33:01Z
last_modified: 2026-08-23T03:33:01Z
status: accepted
decided_on: 2026-08-23
deciders: operator (reasoning-substrate bundle), implementing agent
scope: subsystem
tags: schema, release-wave, format-generation, reasoning-substrate, deferrals
---

# Ship the reasoning substrate as one schema-language wave and defer the named remainders

## Decision
We chose to land the five reasoning-substrate forms (conditional `required_outgoing`, `must_reach`, `acyclic_sets` plus `status_propagation.rel_types`, `signals`, `relationships.labelling`) as ONE schema-language generation released together as engine 0.10.0, with the release due within days of the last form per the cadence principle's wave clause. During the wave the new keys lived only in test fixtures; the docs guide, one example schema, and the `memstead schema new` scaffold gained them in the same cut that releases them, alongside the forward-compatibility note: packages declaring wave keys need engine 0.10.0 or later, older engines refuse them at parse ([[engine--schema-definition-format]]).

## Context
The wave is the engine half of the accountable-reasoning program; each form was grounded in the 2026-08-21 argument-schema experiment and the claimstead-bootstrap research, and every form is generic by construction (planning, obligations, and spec models exercise them as readily as argumentation). The 2026-08-23 deep review re-cut the bundle: option D of the briefing (all five forms), keys pinned per plan, `distinct_by` moved out, the neighbour filter moved in, signal levels split from constraint severity.

## Consequences
- Deliberately deferred, each with its recorded reason: the metadata-tuple conflict signal (needs a new `signals` kind, a format event, so a later wave); `distinct_by` (needs an engine-owned per-edge provenance sidecar keyed source/rel_type/target first, then `distinct_by: mutation`, never `actor`, because the recorded Actor is a caller category and transport is not identity); cardinality variants (`exactly_one` / `at_most_one`, routed in the backlog's constraint-vocabulary cluster); pairwise endpoint shapes; Carneades-style expected-label regression tests (briefing mechanism 6, not in the wave).
- The polarity boundary is pinned by a per-form test: `ConstraintSeverity` keeps exactly two members, and a `notice` in any severity slot of any form refuses at load; `SignalLevel` is its own enum.
- No truth computing landed anywhere in the wave: every form reports structure or refuses writes on structure; nothing scores, weighs, or decides which content is right.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-five-form-constraint-vocabulary]]
- **REFERENCES**: [[engine:schema-definition-format]]
- **REFERENCES**: [[require-an-outgoing-edge-conditionally-on-a-metadata-value]]
- **REFERENCES**: [[declare-reachability-obligations-that-walk-to-terminal-types]]
- **REFERENCES**: [[guard-cycles-and-walk-taints-over-relation-sets]]
- **REFERENCES**: [[serve-declared-aggregate-signals-as-counts-with-thresholds-and-evidence]]
- **REFERENCES**: [[serve-the-grounded-labelling-as-an-observation-with-its-evidence]]

## Options

- Releasing per form (five releases): rejected by the wave clause, one format generation, one release.
- Teaching the keys in docs as each form landed: rejected, the tree must not teach keys the released binary refuses; docs land in the release cut.
- A minimum-engine-version marker on schema packages: not built this wave; the parse refusal (`deny_unknown_fields`) plus the cadence rule remain the standing mitigation, and the changelog states it.

## Notes

Wave landed 2026-08-23 across reasoning-substrate plans 01 to 05 (public commits 88474a0, b7290f1, caeb54f, 0ae0505, a55398f), each independently graded. Per-form decisions: [[engineering--require-an-outgoing-edge-conditionally-on-a-metadata-value]], [[engineering--declare-reachability-obligations-that-walk-to-terminal-types]], [[engineering--guard-cycles-and-walk-taints-over-relation-sets]], [[engineering--serve-declared-aggregate-signals-as-counts-with-thresholds-and-evidence]], [[engineering--serve-the-grounded-labelling-as-an-observation-with-its-evidence]]. The 0.10.0 release leg, push, and tag are tracked by plan 06.
