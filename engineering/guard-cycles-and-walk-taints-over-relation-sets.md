---
type: decision
created_date: 2026-08-23T01:54:06Z
last_modified: 2026-08-30T00:32:30Z
status: accepted
decided_on: 2026-08-21
deciders: operator (reasoning-substrate bundle, confirmed 2026-08-23), implementing agent
scope: subsystem
tags: schema, acyclicity, relation-sets, status-propagation, cycle-gate, reasoning-substrate
---

# Guard cycles and walk taints over relation sets

## Decision
We chose to let acyclicity and status propagation operate over relation SETS where each spoke a single rel-type before. The schema manifest declares `relationships.acyclic_sets` (inline lists of two or more declared rel-type names, non-overlapping): a write closing a cycle in a set's union subgraph refuses with the existing RELATIONSHIP_CYCLE error through the single shared gate on every edge-writing verb, and the payload additively echoes `acyclic_set` plus one rel-type per hop (`existing_path_rel_types`), so the refused path may honestly mix rel-types. The per-definition `acyclic` flag keeps its exact meaning and payload. The boot sweep generalises to cycle spaces (one per acyclic flag, one per declared set) so a package imported with a mixed-type cycle boots into the same state the write path defends. The `status_propagation` constraint accepts `rel_types` alongside the still-legal `rel_type` (exactly one per declaration); the taint walk crosses rel-type boundaries along the union ([[engine--graph-health-report-surface]], [[engine--schema-definition-format]]).

## Context
Third form of the reasoning-substrate wave. The 2026-08-21 argument-schema experiment hit the forced choice head-on: a support chain alternating two rel-types (premise into inference, inference into conclusion) has a circular-reasoning cycle no single-rel-type subgraph contains, so per-rel-type acyclicity cannot refuse it. The workaround (collapsing to one rel-type) made premise obligations inexpressible: either honest modelling or cycle protection, not both. Generic capability: any pair of hierarchies that jointly must stay acyclic, any taint that travels heterogeneous edges.

## Consequences
- One cycle vocabulary: the set refusal reuses RELATIONSHIP_CYCLE and its recovery shape; single-rel-type refusals stay byte-identical, and every consumer surface (MCP servers, CLI) carries the additive fields.
- A rel-type may appear in at most one set; the loader refuses overlap, single-member and empty sets, and undeclared names.
- Cycle refusal stays write-time (a cycle is completed by exactly one identifiable write); no health-only mode for the set form.
- Part of the declared release wave. Corrected 2026-08-30: the wave shipped. `relationships.acyclic_sets` is released under 0.10.0 (2026-08-23) in the public changelog, so the pre-tag embargo this bullet described has lapsed. No built-in schema declares the keys; the schema scaffold (`memstead schema new`) documents them in its commented template.
- Schemas without the declarations keep byte-identical responses and behaviour.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **REFERENCES**: [[engine:graph-health-report-surface]]
- **REFERENCES**: [[engine:schema-definition-format]]

## Options

- A workspace-global acyclicity toggle: rejected, acyclicity is schema semantics, not workspace policy.
- Deriving the set implicitly from all acyclic-flagged rel-types: rejected, it would silently merge independent hierarchies (PART_OF and SUPERSEDES must not share one cycle space).
- Prescribing the single-rel-type modelling trick instead: rejected, the experiment showed it costs premise obligations and per-pair endpoint honesty; a convention that punishes correct vocabulary is the engine's defect.
- Letting `rel_type` accept a string or a list (untagged union): rejected, it muddies the generated meta-schema; a second key with one value space each is plainer.

## Notes

Landed 2026-08-23 with reasoning-substrate plan 03; keys recorded under [Unreleased] in the public changelog; released only with the wave (plan 06). The boot sweep's generalisation to cycle spaces deduplicates drops for edges sitting in two spaces at once (rel-type flagged acyclic AND inside a set).
