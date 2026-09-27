---
type: decision
created_date: 2026-09-20T02:54:30Z
last_modified: 2026-09-27T17:01:15Z
status: accepted
decided_on: 2026-09-20
deciders: operator (in-session, following the briefing of 2026-09-20), memstead-71
scope: system
tags: proposal, fork, merge, provenance, anchors, archive
---

# Build proposals as forks with a recorded ancestor, merged entity-wise under two identities

## Decision
We will build the pull request for mems as four extensions of mechanisms the engine already has, never as a proposal system of its own: a fork is a mem created from another mem's branch at a sha with the ancestor recorded in its config; the diff is the existing two-ref diff read three ways (ancestor, owner tip, fork tip); the merge is the existing batch apply committed against the owner tip's expected parent, carrying the proposer's identity as author and the merger's identity in a second, additive commit trailer, followed by a check record of the merger on every adopted entity; the record of every proposal and its per-entity disposition rides the target branch as an engine-owned sidecar. Beside it, two extensions that stand on their own: an anchor may carry a verbatim span that is checked inside the supplied observation at write time and adjudicated on presence at observation time, and a sealed archive carries the mem's check records as an optional member so a served page can show them. Fork, merge and list are CLI-only; the brief alone is also an MCP read tool (`memstead_proposal_brief`, the twenty-first tool), the operator's explicit exception of 2026-09-20 to the less-surface principle so the owner's agent can pre-fill the review.

## Context
A project that publishes reviewed claim collections on the engine walked a proposal flow once on 2026-09-02 (its Test A, findings M1 to M6) and found the missing third: without a fork the cross-mem diff showed thirty false deletions, the merge lost the proposer, a derived-from anchor pinned at verify time not write time, partial adoption was hand surgery, a rejection lived only on the proposal side, and edges into the target needed a grant per pair. The analysis of 2026-09-20 (an operator briefing, disposable) checked each block against the code: the branch, transport, tree-at-ref validation, diff with ripple, per-mem atomic batches with the full write gate, commit trailers with one identity, the check ledger, the [[engine--preparation-registry]]'s quoted-phrase preparation and the archive validator's tolerance for unknown meta members all exist; what is missing is four small pieces. The [[engineering--one-writing-machine-per-mem-repo-a-cross-machine-graph-merge-waits-for-a-project-that-needs-it]] decision fixed the merge shape for the day a project needs it and named that day as its own trigger; the claim-collection project is that project, and the shape is kept: entity-grain three-way, typed refusal on an entity edited on both sides, the validator as the gate, no textual merge. The cadence hold to about 2026-10-07 and the surface freeze are respected because no schema language changes and no generated reference is added beyond the CLI reference; four CLI verbs and one MCP read tool for the brief are the operator's explicit addition (the tool decided 2026-09-20 as the one exception to the less-surface principle).

## Consequences
- Order of work: the quoted-span check and the sealed check records first (each extends existing verbs), then the fork verb, then brief, merge and record as one bundle built against the claim-collection project's first real case; nothing of the last three is built before that case exists.
- The 2026-09-10 rule stands for what it was about: one writing machine per lineage, forks across clones healed by reset plus rebuild. A proposal fork is a second lineage with a recorded ancestor and is merged, never healed.
- Partial adoption is per entity; adopt-with-changes lands as two honest commits, the proposer's version under the proposer's identity and the merger's edit under the merger's, so the independence reading needs no third author.
- A conflict (an entity edited or deleted on both sides since the ancestor) is reported with all three versions and the sections that differ, and refuses plain adoption; a human supplies the merged body or rejects. No automatic semantic merge.
- The proposal record lives on the target branch as a sidecar, travels into archives, and lets a brief flag a re-proposal of a rejected entity by content hash or id. Nothing of a fork or its record is served before the merge.
- Span comparison runs on a canonical form (NFC, collapsed whitespace, de-hyphenation at line breaks, typographic quotes and dashes to ASCII, ligatures resolved) and never on digits, units, case or word order; no similarity threshold.
- The sealed check records add an optional archive member under the provenance redaction classes; the published format number stays.
- Costs accepted: four new CLI verbs and one MCP read tool (the brief) against the surface freeze's spirit, a third and fourth engine-owned sidecar, a sidecar version bump for spans, and a merge that is serial per owner by design.
- Server-side concerns (key-to-handle verification, rate limits, a pre-receive hook that refuses a proposal tree failing the mechanics check) are infrastructure outside the engine; the engine offers the tree check as an exit-code verb.

## Relationships
- **SUPERSEDES**: [[one-writing-machine-per-mem-repo-a-cross-machine-graph-merge-waits-for-a-project-that-needs-it]]
- **MOTIVATED_BY**: [[engine-owns-mem-repo-state]]
- **INFORMED_BY**: [[caller-declared-identity-is-the-independence-gates-only-comparator]]
- **INFORMED_BY**: [[mcp-tool-surface-stays-small]]
- **MOTIVATED_BY**: [[engine:git-transport-and-history-surface]]
- **CONSTRAINS**: [[engine:anchor-primitive]]
- **CONSTRAINS**: [[engine:mem-archive-export-surface]]
- **REFERENCES**: [[engine:preparation-registry]]
- **REFERENCES**: [[one-writing-machine-per-mem-repo-a-cross-machine-graph-merge-waits-for-a-project-that-needs-it]]

## Options

- A proposal system of its own (proposal objects with a lifecycle, their own store, diff, apply path, provenance and verbs, likely as MCP tools): rejected, it duplicates diff, validation, provenance and atomicity that exist, creates a second place of truth beside branch history and ledger, and is the kind of new surface the 2026-09-10 freeze names.
- A schema field type for quoted spans binding a field to the entity's anchor: rejected for now, it changes schema language and makes a release due within days against the cadence hold; it can sit on the anchor attribute later.
- Format bump for the sealed check records: rejected, the validator tolerates unknown meta members, so an optional member reaches old readers as absence.
- `mem init --from` instead of a fork verb: rejected, init reads as a template, a fork is a lineage with an ancestor.
- Section-level dispositions: rejected, the diff carries whole bodies, and section verdicts let cross-section constraints slip; the brief names differing sections as comfort.
- Proposal record as workspace state (like the findings store) or as entities in a process mem: rejected as the record of truth, the first does not travel with the mem and the second needs a new entity type; a project may keep prose beside it.
- Four extensions of existing mechanisms, CLI-only except the brief, which is also an MCP read tool (chosen).

## Notes

2026-09-27 amendment: a merge is a decision record before it is a content change, so the record entry lands even when nothing is adopted. Implementation defect found by a project that uses the engine and reproduced on the engine's own fixtures: the record sidecar was written inside the commit loop that lands one commit per adopted proposer, so a disposition file rejecting every entry landed no commit and no record entry, the target branch kept no trace of the refusal, the brief could not mark a re-proposal of the rejected content, and a reader of the branch could not learn the fork was disposed. Fixed the same day: with nothing adopted the merge lands one commit on the target branch under the merger's identity carrying the record alone, parent-pinned like every merge commit, with the proposal trailers and no entity ids; the record entry names the proposer read off the fork commits for every entry it can attribute, rejected ones included, and only an adopted entry's missing identity refuses. The fork stays untouched, as the decision states. Deciders: memstead-e9 under the engine-changes rule; the reporting side confirmed the merger as the commit's identity.


2026-09-27 amendment, the owner's side of a hosted proposal path: `mem fork <source> <name> --remote <r> --adopt`. The remote form of the fork verb means the source lives on the remote and makes an empty fork at its tip, so it could not take in a proposal branch a contributor had pushed, and a plain git remote without a registry ref was refused; the reporting project's workaround (local fork, fetch, branch-reset) puts a retargeted base against an un-retargeted tip and only held because the sample tree carried no anchor rows. The adopt form fetches the branch of the fork's own name, takes its merge-base with the local source branch as the ancestor (refused when there is none), validates the fetched tree against the local pin as pull does, derives the config from the local source, and creates the branch at the fetched tip with no commit of its own: an adopted tree is the proposer's. Its base is the engine's fork commit when the branch carries one, absent otherwise (the brief already reads such a fork against its ancestor). The remote needs no registry ref, in keeping with the consequence above that server-side concerns stay outside the engine: a server that writes proposal commits itself must write the engine's commit shape (the engine subject or an Entities trailer, plus Identity and Role trailers), which the changelog states verbatim. Options: (a) the flag on the fork verb, chosen, the result is a fork in every respect; (b) a separate verb, rejected as one more verb against the surface freeze's spirit; (c) no engine change, rejected on the anchor fragility. An explicit ancestor override was not built: merge-base is the ancestor until a case shows otherwise. Deciders: operator (in-session, 2026-09-27), memstead-e9.
