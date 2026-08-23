---
type: decision
created_date: 2026-08-23T18:07:56Z
last_modified: 2026-08-23T18:07:56Z
status: accepted
decided_on: 2026-08-23
deciders: operator (consolidation decision I), implementing agent
scope: system
tags: bindings, sync, baseline, code-mems
---

# A stale code mem is re-baselined at the commit it reflects, never at HEAD and never rebuilt

## Decision
We will re-baseline a binding whose sync state has rotted (a token in the wrong repo, a commit that exists nowhere, a key in a retired shape) at the commit its mem actually REFLECTS, in the live key shape, and then carry the mem forward through the sync loop batch by batch. Re-anchoring to the current HEAD is forbidden: it would declare hundreds of unexamined changes synced and turn the fidelity report into a lie. A from-scratch rebuild is forbidden too: the code mems carry hand-curated prose and cross-mem edges a rebuild would destroy. Concretely, `engine/graph` and `plugin/graph` were re-baselined at `10b7d85` (the last public commit before 2026-07-05, the content their mems reflected) and `registry/graph` kept its still-valid token under the live key; the cleared reference-facet keys are re-established by each binding's next pass.

## Context
On 2026-08-23 the three binding-built code mems described the world of 2026-07-04: the engine mem's sync token named a commit that exists in no repo (it predated the public/ repo's genesis), the plugin mem's token was an outer-repo commit (the wrong repo for its source), and all keys sat in the retired gen-1 shape the binding layer no longer reads. Nothing ran the loop. The honest state was "synced as of the old commit", and only a baseline that says so lets the brief present the true delta.

## Consequences
- The first brief after the re-baseline presents the real delta (deleted crates first), and every batch worked is a batch of actual judgement; nothing is declared synced unexamined.\n- The loop is long: the engine binding's delta spans hundreds of artifacts, so sessions record batches consumed and the plan carries a bounded exit instead of a finish-line fiction.\n- The keys live in the binding-id shape (`<mem>/<stem>/<facet>#synced`), so the rotation, briefs and verify all read them.\n- Cost: sync work proportional to the true drift; that cost was always owed, the baseline just stops hiding it.

## Options

- Re-anchor to HEAD: rejected; it forges coverage and the verify report would bless a mem that still describes July.\n- Rebuild from scratch: rejected; 280 entities of curated prose and inbound cross-mem edges would be destroyed for no gain in truth.\n- Re-baseline at the reflected commit and walk forward: chosen.

## Notes


