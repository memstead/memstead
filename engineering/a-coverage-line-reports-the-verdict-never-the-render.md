---
type: decision
created_date: 2026-09-03T12:46:41Z
last_modified: 2026-09-03T12:46:41Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C10 executing session
scope: subsystem
tags: health, coverage, gates
---

# A coverage line reports the verdict, never the render

## Decision
We will let a verdict surface's coverage line report what its VERDICT answers for, never what the pass happened to render. Rendering an axis is not examining it. Concretely, the health line files `anchors` as advisory on every pass, the promotion of an opt-in axis into the examined bucket is removed at every call site, and the line renders from the registry declaration alone so that CLI markdown, CLI JSON and MCP cannot disagree.

## Context
The composer promoted `anchors` into `examined` whenever the axis was included, while the strict exit fails on an unreadable anchors sidecar alone and its help says drifted anchors stay advisory. A gate trusting the coverage line therefore believed the health verdict policed anchor drift when the verify surfaces carry that statement. The registry row already declared the axis advisory with the right reason; only the promotion overrode it.

## Consequences
- The rendered line agrees with the strict exit on every fixture: no fixture files an axis as examined while its findings leave the gate green.
- The tradeoff accepted: the composer's "rendered this pass, therefore examined" rule is gone. This axis was its only instance, so no behaviour is lost, but a generalisation that appeared to exist did not survive contact with a surface whose verdict is narrower than its render.
- A reader still needs two commands to police structure and drift. That division is deliberate and now stated in one place instead of contradicted in another.
- The rule generalises: any surface that renders more than its verdict answers for must file the difference as advisory.

## Options

- Narrow the promotion so the axis stays advisory: CHOSEN.
- Widen the strict exit to fail on drift, making the line true as written: rejected on evidence rather than taste. The one real strict consumer includes the anchors axis in its strict run AND runs the fidelity verify separately against a drifted-anchor ceiling, so the sole caller that could have wanted it had already arranged the opposite; and every gate reading the health verdict would start failing on a mem with one drifted anchor.
