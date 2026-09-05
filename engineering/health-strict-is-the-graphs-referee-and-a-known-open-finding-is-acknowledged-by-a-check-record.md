---
type: decision
created_date: 2026-09-05T15:34:58Z
last_modified: 2026-09-05T15:46:11Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: health, checks, referee, ci
---

# health --strict is the graph's referee and a known-open finding is acknowledged by a check record

## Decision
memstead health --strict evaluates one fixed set whatever --include says (integrity, anchors, stale, missing_required_outgoing, constraints, signals; the report names it under strict.evaluated) and exits 1 on any unacknowledged entity finding, any configuration defect, and any stale acknowledgement; stale entities, drifted anchors and generation hints stay advisory. A known-open finding is acknowledged by a check record on its entity whose finding.code names the health condition, with verdict failed and a method naming the owner and the plan that closes it; the finding is then reported as acknowledged and does not fail the run, and a later ok on the same condition withdraws it. An acknowledgement that still stands while its finding no longer occurs is STALE_ACKNOWLEDGEMENT, a condition naming the record, so the acknowledged set shrinks with the repairs. Finding codes spelled UPPER_SNAKE are the engine's namespace and must name a health condition (refused INVALID_CHECK_FINDING otherwise, and an acknowledgement without a method refuses too); any other spelling stays the checker's own vocabulary, which narrows [[engineering--a-check-record-carries-a-structured-finding-and-admits-an-open-x-kind-the-engine-never-interprets]] on exactly the codes the engine now interprets and leaves every other code open.

## Context
From 2026-08-23 the dogfood graph's referee was a workspace script around memstead health --strict: it derived one finding line per defect, compared the set against a residual list kept beside the workspace, and was green only on set equality, so a new defect and a repaired residual were both red. The comparison was the script's whole job; the residual list was the acknowledged set kept by hand, one line per known-open defect with its owner. The 2026-09-04 census (posture C) moved the comparison into the engine: the check substrate already carried kinds, findings and identities, and an acknowledgement is a checker's verdict on a known defect. The strict set stays what the script requested; the anchor-drift ceiling the script applied to the flagship verify is that verify's own --fail-on-findings.

## Consequences
The Graph health workflow runs memstead health --strict and the flagship projection verify --full --fail-on-findings --fail-on-inconclusive and nothing else; the referee script, its replay test, the residual list and the recorded pre-repair fixture are gone. The one declared residual of 2026-08-29 (the institute evidence entity's missing STRENGTHENS/WEAKENS/VALIDATES/CONTRADICTS edge, owner the operator) is now a check record on that entity. The HEALTH_STRICT_VIOLATIONS envelope names the violating codes with counts instead of section labels. Rejected: an engine-side residual file (the list the script kept, moved one directory) and keeping the script around the engine call (its job was the comparison, which the engine now owns). The claims sweep the script also ran leaves the lane until the claims register becomes a verified binding (the bundle's sixth plan). Configuration defects (a pin mismatch, an unbacked mount, a rotted package) are never acknowledgeable: none belongs to one entity, and each is a repair, not a known-open finding.

## Relationships
- **REFERENCES**: [[a-check-record-carries-a-structured-finding-and-admits-an-open-x-kind-the-engine-never-interprets]]
