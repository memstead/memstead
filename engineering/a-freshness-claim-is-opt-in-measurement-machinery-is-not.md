---
type: decision
created_date: 2026-09-03T12:46:41Z
last_modified: 2026-09-03T12:46:41Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C6 executing session
scope: subsystem
tags: projection, verify, gates
---

# A freshness claim is opt-in, measurement machinery is not

## Decision
We will split a measuring surface's writes by what they ASSERT, not by where they land. A write that claims something about freshness is opt-in: the `<binding>/<facet>#verified` token a later run reads to decide whether verifying is due moves only under `projection verify --advance`, and a bare run says on both surfaces that it did not move it. A write that is the measurement's own machinery is never gated: the findings store, which is the verify surface's state outside the mem, and the prepared-hash backfill that stamps observed hashes onto hash-less anchors, without which an anchor never leaves `recheck` and drift stops being adjudicated at all.

## Context
Filed after a grader verified a binding in order to READ it, bumped the freshness baseline, and had to revert its own bump by hand. The obvious fix, gating every write the run makes into the mem, was implemented and reverted within one session on evidence: seven projection tests moved from reporting drift to reporting clean, because a withheld backfill leaves every hash-less anchor unadjudicated. The two writes look alike from outside (both land in the destination mem, both were called bookkeeping) and are not alike at all.

## Consequences
- A gate, a grader or a CI job that verifies in order to read leaves the destination mem's config byte-identical.
- The tradeoff accepted, stated rather than buried: "a bare verify writes nothing into the mem" is still not literally true, because a run with hashes to backfill commits the anchors sidecar. What holds is narrower and honest: it does not move the freshness token, and on a settled mem it writes nothing at all.
- A caller that wants the baseline current must ask for it; a workflow that never does keeps re-verifying a binding the selection loop sees as never verified. The maintenance recipes therefore pass the flag, and a gate does not.
- Any future surface that both measures and records must ask, per write, whether it asserts a claim or does the measurement, and gate only the first.

## Options

- Gate the freshness token only, keep the measurement writes ungated: CHOSEN.
- Gate every mem-facing write behind the flag: rejected on measured evidence, not argument. It satisfies the literal wording while defeating the goal, because the measurement stops working.
- Keep the bump and document that verify is not read-only: rejected. The entry was filed precisely on a gate dirtying what it checks; documenting the behaviour does not stop the commit someone must revert.
