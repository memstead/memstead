---
type: decision
created_date: 2026-08-23T03:10:21Z
last_modified: 2026-08-30T00:24:19Z
status: accepted
decided_on: 2026-08-21
deciders: operator (reasoning-substrate bundle, confirmed 2026-08-23), implementing agent
scope: subsystem
tags: schema, labelling, grounded-semantics, attack-graph, chain-shape, reasoning-substrate
---

# Serve the grounded labelling as an observation with its evidence

## Decision
We chose to let a schema name which of its rel-types constitute attack (`relationships.labelling.attack`) and have the engine serve the grounded labelling of that attack graph: unattacked entities `accepted`, targets of an accepted attacker `defeated`, entities whose attackers are all defeated `accepted`, the rest `undecided`. Of the entire argumentation-semantics literature this is the one computation that is parameter-free, unique, polynomial, and explainable by construction, and also the most sceptical of the polynomial semantics: one unanswered attack defeats a well-supported claim, so the brittleness is served with its evidence (a defeated label always names its accepted direct attackers, an undecided one the open attacker set), never hidden. The labelling runs per mem over a pinned graph (non-stub entities; attack edges with both endpoints in the mem; cross-mem attack edges excluded and counted), memoised beside the community memo and invalidated by the same single reset, and is served on entity reads (`_labelling` envelope key, `_label` frontmatter, `## Labelling` section) and the include-gated `labelling` health axis ([[engine--graph-health-report-surface]], [[engine--entity-read-projection-surface]]). An optional `support` walk (the `must_reach` grammar) adds chain-shape statistics: depth, branching, terminal share, and the defeated/undecided counts on the support subtree.

## Context
Fifth and last form of the reasoning-substrate wave. The named limit is deliberate: the labelling is support-blind (Dung's framework applied to entities, not ASPIC+ structured arguments); a defeated inference does NOT flip the conclusion it supports, because any closure along support edges would be a semantics choice (necessary, deductive, evidential support all differ) of exactly the kind the engine refuses. The reader sees the defeat through `defeated_in_support` and the supporter's own label. A label is a reported observation: the schema's own authored status and the computed label may disagree, and that disagreement is precisely the useful signal; reconciling it is agent work.

## Consequences
- Labels are never stored, never gate writes, never sync into authored fields; being attacked is the normal life of a claim, not a defect.
- Everything richer is refused: preferred/stable/semi-stable (non-unique or NP-hard), weighted/gradual scores, audiences, preferences. Unknown declaration keys refuse via the format's posture.
- The labelling memo's reset lives inside the community-memo invalidation, so every mutation site, drift reload, quarantine transition and apply-commit invalidates both without a second call.
- Cross-mem attack edges are excluded and counted (`cross_mem_edges_excluded`): the attack set is schema vocabulary and two mems may pin two schemas; guessing is worse than honest exclusion.
- Part of the declared release wave; keys ship with the close-out; until the tag no built-in, example, or scaffold emits them.
- Schemas without the declaration keep byte-identical responses everywhere.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **INFORMED_BY**: [[serve-declared-aggregate-signals-as-counts-with-thresholds-and-evidence]]
- **REFERENCES**: [[engine:graph-health-report-surface]]
- **REFERENCES**: [[engine:entity-read-projection-surface]]

## Options

- Preferred / stable / semi-stable semantics: rejected, non-unique or NP-hard; a non-unique label cannot be served as one observation.
- Closing defeat along support edges: rejected, every bipolar extension picks one of several support semantics and the choice changes labels; the shape count gives the reader the fact without the engine picking.
- Auto-syncing an authored status field from the label: rejected, the engine would be writing judgment into state.
- Blocking writes that create defeated entities: rejected, refusal would punish honest recording of disagreement.
- A workspace-global labelling across mems: rejected, exclusion plus a count beats a guess.
- Materialising full attacker chains: rejected, the direct accepted attackers are the evidence and each attacker is one read away.

## Notes

Landed 2026-08-23 with reasoning-substrate plan 05; the keys shipped with the wave as engine **0.10.0** (public CHANGELOG `## [0.10.0] - 2026-08-23`, whose forward-compatibility note names `relationships.labelling`), so they no longer sit under [Unreleased]; no built-in schema or example declares the key, only the `memstead schema new` scaffold documents it (verified 2026-08-30). Shape depth is the max level of the visited-set-bounded breadth-first walk, exact on tree-shaped support.
