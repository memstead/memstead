---
type: decision
created_date: 2026-08-28T15:52:54Z
last_modified: 2026-08-30T00:32:29Z
status: accepted
decided_on: 2026-08-28
deciders: graph-plans-01 executor (operator-released bundle)
scope: component
---

# Gated status transitions are a generic constraint kind: spike verdict yes

## Decision
We will build the gated-transition completion rule as a generic schema constraint kind (working name transition_requires_checks: field, to_value, rel_types, direction, required check state; the shipped form spells the relation-set key `relationships`, not `rel_types`; noted 2026-08-30), evaluated at write time beside the existing declared-constraints pass. The graph-plans 01 spike verdict is YES: the feature is expressible with existing substrate and carries no planning semantics.

## Context
The planning@0.5.0 execution contract needs 'transition to complete requires a check record on every VERIFIES-linked criterion'. Spike question (graph-plans plan 01): can this be a genuinely generic engine feature, usable by any schema, never a pseudo-generic lifecycle walk? Verified against the live tree 2026-08-28: the declared-constraints pass evaluates at create/update with block/warn tiers (mutation/create.rs, ops/health.rs unsatisfied_constraints), incoming-edge enumeration exists (delete refusals), and per-entity derived check state exists hash-staleness-aware (check_ops.rs entity_check_state). The one signature cost: the shared evaluator needs check-ledger access (workspace root), which both mutation and health callers have.

## Consequences
- The completion gate is a schema-declared, engine-refused constraint. Corrected 2026-08-30: this bullet used to name the interim, with the skill enforcing the rule until the constraint was built. `transition_requires_checks` shipped in engine 0.14.0, the workspace planning@0.5.0 schema declares it, and `memstead gates` renders the standing of every declared gate; the skill-side interim is retired.
- Plan 03's walk brief becomes a thin renderer over engine state (the engine-rendered brief family precedent), per the pre-drawn yes branch.
- The constraint gates on recorded check state (checked_ok, hash-fresh), deliberately NOT on the independence label: independence stays a measurement (health checks axis), per the plan-13 trust model.
- A workspace-less engine (no check ledger) derives never_checked and would always refuse such a transition: documented edge, same posture as checks generally.
- Trade-off accepted: one more closed constraint kind in the meta-schema vocabulary.

## Options



## Notes

Revisit if the implementation cannot keep the evaluator single (a second constraint-evaluation regime would violate the one-implementation-per-check rule): then the verdict flips to skill-side.
