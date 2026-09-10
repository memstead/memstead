---
type: decision
created_date: 2026-09-10T14:08:00Z
last_modified: 2026-09-10T14:08:00Z
status: accepted
decided_on: 2026-09-10
deciders: operator (in-session), implementing agent
scope: system
tags: merge, multi-machine, mem-repo, transport, scope
---

# One writing machine per mem-repo; a cross-machine graph merge waits for a project that needs it

## Decision
We will not build an entity-wise merge for mem-repo lineages that diverged across machines. The operating rule is one writing machine per mem-repo: several agents on one machine coordinate through the serialized branch commits and the per-entity optimistic hash lock, and a second machine that has written anyway is healed with the recorded procedure, one lineage wins and derived content is rebuilt from its sources ([[engineering--heal-the-two-machine-mem-repo-fork-by-remote-lineage-reset-plus-source-rebuild]]). The decision is revisited only when a project that uses the engine must write to one mem-repo from two machines; until then no backlog item carries it.

## Context
The 2026-09-10 review of prune separated two questions the merge word had fused. Prune has no merge to make (a source artifact and the entity about it share no ancestor). Two agents changing one entity on two clones of one mem-repo do: the fassung before both changes is in git, so a three-way merge at entity and section grain is well defined, and beyond text a graph merge needs the validator as a gate (one side may delete an entity the other links to) and duplicate detection (two agents create the same subject under different ids). The engine covers the same-machine case by avoidance and the folder-mem git-conflict case by a side-choosing door ([[engineering--merge-conflicts-get-an-engine-door-the-agent-judges-the-engine-writes-the-guards-stay-closed]]); the cross-machine case surfaced once, a sixty-day fork between the operator's two machines in July and August 2026, and was healed by reset plus rebuild. Building the merge is estimated at one to two autonomous half-day sessions for the entity-wise pull with typed refusal on double edits and validation as a gate, and another for duplicate detection. The operator's direction on 2026-09-10 is to stop widening the engine and to let requirements arrive from the projects that use it.

## Consequences
- No open item: the backlog carries this as a watching line with its trigger, not as work.
- A fork that recurs is healed by the recorded procedure; hand-authored entities on the losing lineage are salvaged through engine verbs, derived content is rebuilt.
- When the trigger fires, the design order is fixed here so the session that builds it does not re-decide: entity-wise three-way merge on `pull` with a typed refusal for an entity edited on both sides, then post-merge validation of the whole store as the gate, then duplicate candidates proposed to the agent in the brief.
- The cost accepted: a second writing machine remains unsupported, and a user who runs one learns it from the healing procedure, not from a merge.

## Relationships
- **INFORMED_BY**: [[heal-the-two-machine-mem-repo-fork-by-remote-lineage-reset-plus-source-rebuild]]
- **REFERENCES**: [[merge-conflicts-get-an-engine-door-the-agent-judges-the-engine-writes-the-guards-stay-closed]]
- **REFERENCES**: [[heal-the-two-machine-mem-repo-fork-by-remote-lineage-reset-plus-source-rebuild]]

## Options

- Build the entity-wise pull merge now: rejected, no live payer and the engine is to stop widening.
- Textual git merge per branch: rejected earlier for the sixty-day fork, it surfaces the same conflict masses without the validator.
- One writing machine per mem-repo with the healing procedure as fallback (chosen).
