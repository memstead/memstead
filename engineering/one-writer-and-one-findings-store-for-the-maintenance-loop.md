---
type: decision
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:43:05Z
status: accepted
decided_on: 2026-07-10
deciders: operator (dasboe)
scope: subsystem
tags: projection-pipeline, verify, sync, e3b, engine
---

# One writer and one findings store for the maintenance loop

## Decision
The projection pipeline's maintenance loop is resolved into **one writer and one findings store**. *Ingest builds* (discovery, one-shot). *Verify measures* — deterministically, mutating nothing, writing durable findings into an engine-owned store keyed `(hash(D), source_head)`. *Sync is the sole maintenance writer*, receiving both the cursor slice and the open findings through ONE rendered brief and enacting repairs only via an agent acting on that brief through the normal mutation surface. Prune rides the same sync brief as proposals; the tier-1 fidelity report and `status` rollup are read-only dashboards over the same computation. The old independent refinement writer (and its 10-minute temp-findings handover) is retired; only its rotation machinery survives, repurposed for verify-sample scheduling.

## Context
Before E3b the subsystem had two would-be writers (refinement-as-writer + a future sync/repair) with two conservatism prose sets and an evaporating temp-file handover — an incoherence the design phase flagged. Findings needed keyed invalidation (a declaration edit or source move must mechanically stale them, which schema metadata cannot enforce), engine-write-path atomicity with the measurement pass, and token-budgeted rendering inside briefs/reports — properties a mem-as-store could not give. The plugin `/reconcile` skill's accumulated judgment (five conservatism rules, edge-removal stance, first-sync/adopt framing, rationale-not-changelog, commits-nothing) had to survive the collapse rather than be lost. Built on the anchor primitive ([[engine--anchor-primitive]]) and the v1 binding format / `hash(D)` in [[engine--memstead-base-crate]].

## Consequences
Findings are engine-owned state, not a mem (unless the operator decides otherwise at the pending process-mem decision gate). Verify never mutates the destination mem — the one-writer boundary is enforced structurally (`&Engine` shared borrow). Reconcile's judgment is absorbed into the engine-rendered sync brief and proven carried by a committed reconcile-absorption diff artifact that gates `/reconcile`'s retirement. The five `ingest@0.1.0` process mems become a second home for the same knowledge — opening the process-mem fate as a genuine operator decision (parked as an open operator decision). Downstream: E3b's verify tiers, coverage/accuracy denominators, and the sync brief's drift slices all read this store; the plugin sync/verify routers and non-code pilots consume the briefs.

## Relationships
- **REFERENCES**: [[engine:anchor-primitive]]
- **REFERENCES**: [[engine:memstead-base-crate]]

## Options

Considered keeping the refinement writer alongside sync (rejected: two writers, two prose sets, a temp handover that evaporates after 10 minutes — the incoherence the design phase resolved). Considered findings in the `ingest@0.1.0` process mems (rejected for the verify loop: no keyed invalidation, measurement routed through the mutation/commit-provenance surface, no token-budgeted rendering — though what the mems uniquely hold, agent-authored mined-source judgment ledgers, is why the process-mem fate is a real operator decision, not a mechanical migration).

## Notes


