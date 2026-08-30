---
type: decision
created_date: 2026-08-23T01:20:39Z
last_modified: 2026-08-30T00:32:29Z
status: accepted
decided_on: 2026-08-21
deciders: operator (reasoning-substrate bundle, confirmed 2026-08-23), implementing agent
scope: subsystem
tags: schema, constraints, reachability, must-reach, health-sweep, reasoning-substrate
---

# Declare reachability obligations that walk to terminal types

## Decision
We chose to give type definitions a `must_reach` block: entities of the type must reach at least one non-stub entity of a named set of terminal types, following edges of an inline relation set in a declared direction (`out` / `in`, the vocabulary the store and search already speak), within an optional maximum depth. Evaluation is health-sweep only, on the `constraints` axis ([[engine--graph-health-report-surface]]); the write path never evaluates it, and the loader refuses `severity: block`, because a transitive gap is created by writes on other entities and a refusal would punish the wrong mutation (the same posture status_propagation established). Findings echo the whole declaration so the reader repairs without re-fetching the schema; walks use visited-set discipline, follow cross-mem edges like any edge (the engine's established traversal posture), and end at stubs, which never count as terminals. The incoming direction with depth 1 deliberately covers the required-incoming-edge case, so no separate `required_incoming` form exists.

## Context
Second form of the reasoning-substrate wave. `required_outgoing` checks one hop, so a chain of claims could satisfy its obligations forever by pointing at further claims and never touching ground truth; the assurance-case tradition calls the missing check "every leaf terminates in evidence", and the 2026-08-21 argument-schema experiment produced its concrete failure, the floating leap: an inference with a conclusion and no premises, invisible to health. Generic capability: any reach-a-terminal obligation (a task must trace to a goal, a spec section must ground in a requirement) uses the same form.

## Consequences
- The sweep pays the walk cost; the write path pays nothing. A reverse adjacency index is built once per sweep, and only when some pinned schema declares an incoming obligation.
- Warn tier is the ceiling by design; the loader refuses the block promise rather than load-and-downgrade.
- Condition scope stays closed: reach-a-terminal only. Arbitrary path patterns were rejected; each new pattern must argue itself as its own form.
- Part of the declared release wave. Corrected 2026-08-30: the wave shipped. `must_reach` is released under 0.10.0 (2026-08-23) in the public changelog, so the pre-tag embargo this bullet described has lapsed. No built-in schema declares `must_reach`; the schema scaffold (`memstead schema new`) documents the key in its commented template.
- Schemas without the form keep byte-identical responses and health output.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **INFORMED_BY**: [[require-an-outgoing-edge-conditionally-on-a-metadata-value]]
- **REFERENCES**: [[engine:graph-health-report-surface]]

## Options

- Write-time enforcement with deferred evaluation: rejected, no single write completes a transitive absence; the concept does not type-check.
- A generic path-query language: rejected, same reasoning as the five-form decision; reach-a-terminal covers the assurance, premise, and grounding checks.
- Folding into status_propagation: rejected, propagation walks FROM a tainted entity outward and reports dependents; reachability asks whether a path to a terminal EXISTS. Different quantifier, different finding shape.
- Re-checking cross-mem link policy during the walk: rejected, edges that exist were granted; a second policy check inside a health walk would disagree with the three existing traversals and buy nothing the integrity axis does not already report.

## Notes

Landed 2026-08-23 with reasoning-substrate plan 02; keys recorded under [Unreleased] in the public changelog; released only with the wave (plan 06). Sibling of the conditional-edge form from plan 01: both are edge obligations on a type, declared next to `required_outgoing` rather than as new `constraints:` kinds.
