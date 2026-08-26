---
type: decision
created_date: 2026-08-26T08:10:35Z
last_modified: 2026-08-26T12:26:29Z
status: accepted
decided_on: 2026-08-26
deciders: operator
scope: system
tags: mem-repo, sync, divergence, git-branch, reconciliation, dogfood
---

# Heal the two-machine mem-repo fork by remote-lineage reset plus source rebuild

## Decision
When the dogfood graph's git-branch mems diverge across machines beyond fast-forward reach, the serving position is reset to ONE lineage and the derived content is rebuilt from the sources rather than text-merged. For the 2026-07-04 to 2026-08-26 fork the other machine's lineage (the remote) won for all four mems and the schemas ref: it was fresher on three of four mems and carried the 0.11.0/0.12.0 release work. The losing lineage's unique hand-authored entities are salvaged selectively via engine verbs afterwards; its sync-derived content is deliberately dropped and re-derived by `/memstead:sync --inventory` runs per code-mem binding, because the source code is the authority for derived mems.

## Context
Nothing synchronized the dogfood workspace's mem-repo between the two machines after 2026-07-04; both engines kept committing locally. Measured divergence at healing time (local-only/remote-only commits): engine 345/94, plugin 131/75, registry 111/14, the `__MEMSTEAD` schemas ref 174/80, and `exec-launch-claims` 22/8 with NO merge-base at all (both machines created that mem independently). Both machines ran the same sync loops over the same sources, so 44 to 54 entity files per code mem were touched on BOTH sides. `memstead pull` only fast-forwards, so it could not heal this. The May 2026 multi-engine-coherence work ([[engine:reload-before-operation-coherence]], [[use-optimistic-content-hash-locking-for-all-mutations]]) solved same-machine coherence and explicitly deferred cross-machine write coordination; this fork was that deferred seam materializing.

## Consequences
This machine's local-only lineage left the serving position (preserved via recorded pre-reset SHAs and the reflog). The fidelity/01 knowledge-mem split, which had run only on this machine, was re-applied through engine verbs after the reset: 47 resurrected duplicate entities re-deleted, and roughly a hundred edges and body wiki-links repointed to the `engineering` and `project` successors. Hand-curated local-only launch claims were re-created; derived spec content is re-derived from source by per-binding inventory runs. The named follow-up is a divergence-aware `pull` that merges entity-wise and refuses typed on both-edited entities, so the NEXT divergence is caught small instead of healed wholesale.

## Relationships
- **REFERENCES**: [[engine:reload-before-operation-coherence]]
- **REFERENCES**: [[use-optimistic-content-hash-locking-for-all-mutations]]

## Options

Textual git merge per branch was rejected: with heavy both-side overlap on derived content it produces conflict masses over material the source code already authorizes, and it would hand-mutate the mem-repo against the engine-owns-mem-repo-state rule. Letting this machine's lineage win was rejected: the remote was fresher and carried the release work, while the local side's unique value was a bounded hand-authored set that salvage carries either way. Waiting for the divergence-aware pull feature was rejected for this healing: against a 60-day fork it would only surface the same conflict masses. An entity-level export/install round-trip was rejected as a heavier path to the identical end state.

## Notes

Executed 2026-08-26; the execution trail lives in the private workspace's git log. Pre-reset heads of the discarded lineage: engine 5addfbf34b58, plugin 06e2606b3aa2, registry d35c4b62ce91, exec-launch-claims 0d195fe880c9, __MEMSTEAD 3db5baa8cc2a. One incidental finding: the fidelity/01 commit notes recorded eleven registry decisions as migrated to `engineering--<slug>` while their actual successors live in the `project` mem; the repoints follow the real location.
