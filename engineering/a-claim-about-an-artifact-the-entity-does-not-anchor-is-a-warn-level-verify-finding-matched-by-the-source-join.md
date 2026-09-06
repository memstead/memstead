---
type: decision
created_date: 2026-09-06T09:29:42Z
last_modified: 2026-09-06T09:29:42Z
status: accepted
decided_on: 2026-09-06
deciders: claims-in-sight plan (blind grading of 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: verify, findings, anchors, health, sync
---

# A claim about an artifact the entity does not anchor is a warn-level verify finding, matched by the source join

## Decision
The verify pass records one `unanchored-mention` finding per destination entity and in-scope artifact wherever the entity's prose names the artifact by path and carries no anchor on it, naming the section (the first of several; the detail lists them all). Prose only: a fenced code block is masked before the scan, an inline code span stays visible because it is the ordinary spelling of a path. A path token names an artifact under exactly the two spellings the anchor resolver accepts, the workspace-relative id the binding enumerates and the source-relative form its facet pointer joins onto it, so one file has one identity however it is written; whether a token names an artifact is decided by lookup against the enumerated scope, never by guessing. Whether the entity anchors the artifact is decided by the engine's own reference rule, so a tree anchor over the directory anchors the file. The class is a finding and never a refusal: a write that names an unanchored file lands, the sync brief presents the open ones in their own group, the fidelity report carries the count beside `uncovered` with the two remedies (an anchor via `memstead_update`, or an authored exclusion of the artifact with a rationale, after which mentions of it raise nothing), `memstead status` counts them without turning its verdict, and `memstead health` reports each as an `UNANCHORED_MENTION` warning that stays advisory under [[engineering--health-strict-is-the-graphs-referee-and-a-known-open-finding-is-acknowledged-by-a-check-record]]. The walk always covers the whole enumerated scope, on a sampled pass as on a full one, because it is linear in body size; a mention is therefore re-observed every pass and never carried forward.

## Context
The blind grading of 2026-09-04 exposed a class of mem defect no axis measured: the engine mem's claims about the folder backend (three entities, falsified 2026-07-16) and about title refusal (falsified 2026-08-10) survived three sync passes while verify reported every anchor those entities held as resolving, because the sentence was about a file the entity never anchored. Measured the same day over the engine mem: of 149 entities, 111 name source files, 83 name at least one they do not anchor, 189 such mentions. The first run of the class on 2026-09-06 recorded 123 findings over 62 entities (one per entity and artifact, net of the exclusion ledger and of tree-anchor coverage).

## Consequences
The exposure the blind grading found is now a number with owners: every unanchored mention is listed in the sync brief beside the drifted and uncovered findings and is closed by an anchor or an authored exclusion, and a claim about a file no verify watches can no longer stand invisible for weeks. The cost is a warning per finding in health (123 on the engine mem at first run) until the mem's authors anchor or exclude them; the count never turns a verdict, so no gate goes red on the day the class ships. The findings store gains a third target kind, `mention`, and the fidelity report's coverage block a field, `unanchored_mentions`; consumers branching on the closed class vocabulary see one new value.

## Relationships
- **REFERENCES**: [[health-strict-is-the-graphs-referee-and-a-known-open-finding-is-acknowledged-by-a-check-record]]

## Options

Refuse a write that names a file it does not anchor: rejected, it would refuse legitimate prose (an entity naming a neighbour for contrast) and turn provenance into a tax. Count mentions only at sync time, not in health: rejected, health is where an operator sees a mem's standing without a sync, and the number belongs there. Auto-anchor every mentioned file: rejected, an anchor is a claim of provenance the author makes and the engine may point at what was named but never invent the claim. Match basenames or search the diff's words: rejected for this class, paths under the binding's own two spellings are the deterministic signal; identifier mentions belong to the sync brief's changed-slice presentation.
