---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:43:07Z
status: accepted
decided_on: 2026-07-13
deciders: operator
scope: subsystem
tags: engine
---

# Sync stays brief-driven as measured against a goal-driven arm

## Decision
The maintenance writer of [[engineering--one-writer-and-one-findings-store-for-the-maintenance-loop]] stays **brief-driven**: the engine-rendered sync brief remains the driver, and skills stay thin routers over it. The competing model — hand the agent only a goal plus tools and let it work freely — is **rejected on measurement, not taste**. Three amendments are adopted with the same decision: (1) the brief gains a whole-mem **stale-claim search** step (search the destination mem for claims about the changed facts beyond the slice-anchored entities — the goal arm's one superior behaviour, ported); (2) verify findings must become **head-move-survivable** so open findings reach subsequent sync briefs after the source advances; (3) entity-headcount discipline is a schema/gate concern, not brief prose — conservatism prose demonstrably does not control structure.

## Context
Operator hypothesis (2026-07-13): sync agents are over-directed; deterministic briefs suppress judgment, and goal+tools would serve the three fidelity goals (no gaps, no wrong info, reliable loop) better. Tested by a controlled A/B campaign: 14 controlled iterations, 28 blind proband passes, same model, same CLI surface, same source mutations, ground truth registered before each run. Results: goal arm produced 2 coverage gaps (brief: 0), 5 churn events concentrated on cosmetic rounds (brief: 2), and self-ran the advance ceremony only 7/14 (brief: 14/14); token cost was equal (±2%). Neither arm ever wrote a wrong claim, and on every judgment-heavy round both arms made the same structural choices — the brief suppresses no intelligence the free arm exhibits. The brief lost exactly once: a falsified claim outside the changed slice was left standing (slice-blinkering), which the goal arm's whole-mem search caught — hence amendment (1). Full ledger: a controlled A/B campaign, 2026-07-13 (retained privately).

## Consequences
The prose-freedom debate is closed with data; future brief changes are amendments to a measured baseline, not re-litigations. The one confirmed brief defect (slice-blinkering) and the findings-keyspace leak become the next engine work items, ahead of any new measurement surface (per [[engine--projection-verify-and-findings-store]] this loop's repairs remain agent-enacted through the brief). The full-inventory operation the operator requires — every entity checked against the source, the source checked for full coverage, corrections graph-side only — is designed on top of this baseline: brief-driven repair, verify without caps as the measuring leg. Churn discipline on cosmetic changes is confirmed as a brief property worth protecting: source-unchanged claims are not rewritten, keeping the loop idempotent and drift distinguishable from taste.

## Relationships
- **REFERENCES**: [[one-writer-and-one-findings-store-for-the-maintenance-loop]]
- **REFERENCES**: [[engine:projection-verify-and-findings-store]]

## Options

**Goal+tools writer (operator hypothesis)** — rejected: measurably worse on gaps (2 vs 0), churn (5 vs 2, all on cosmetic rounds — the exact pre-brief failure mode the conservatism rules were written against), and loop self-sufficiency (7/14 vs 14/14), with no measured judgment advantage and no cost advantage. **Brief unchanged (status quo)** — rejected: iteration 12 proved the changed slice steers and blinkers at once; without the stale-claim search the loop leaves falsified claims standing on cross-file semantic changes.

## Notes


