---
type: decision
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-08-21T05:34:42Z
status: accepted
decided_on: 2026-07-10
deciders: operator (dasboe)
scope: subsystem
tags: projection-pipeline, verify, sync, e3b, engine
---

# One writer and one findings store for the maintenance loop

## Decision
The projection pipeline's maintenance loop is resolved into **one writer and one findings store**. *Ingest builds* (discovery, one-shot). *Verify measures* — deterministically, mutating no entity, writing durable findings into an engine-owned store keyed `(hash(D), source_head)`. It is not a pure read: a completed run also backfills observed content hashes onto hash-less anchors and records a `#verified` baseline. "One writer" is about entity content, which only sync touches — not about verify being side-effect-free. *Sync is the sole maintenance writer*, receiving both the cursor slice and the open findings through ONE rendered brief and enacting repairs only via an agent acting on that brief through the normal mutation surface. Prune rides the same sync brief as proposals; the tier-1 fidelity report and `status` rollup are read-only dashboards over the same computation. The old independent refinement writer (and its 10-minute temp-findings handover) is retired; only its rotation machinery survives, repurposed for verify-sample scheduling.

## Context
Before E3b the subsystem had two would-be writers (refinement-as-writer + a future sync/repair) with two conservatism prose sets and an evaporating temp-file handover — an incoherence the design phase flagged. Findings needed keyed invalidation (a declaration edit or source move must mechanically stale them, which schema metadata cannot enforce), engine-write-path atomicity with the measurement pass, and token-budgeted rendering inside briefs/reports — properties a mem-as-store could not give. The plugin `/reconcile` skill's accumulated judgment (five conservatism rules, edge-removal stance, first-sync/adopt framing, rationale-not-changelog, commits-nothing) had to survive the collapse rather than be lost. Built on the anchor primitive ([[engine--anchor-primitive]]) and the v1 binding format / `hash(D)` in [[engine--memstead-base-crate]].

## Consequences
Findings are engine-owned state, not a mem (unless the operator decides otherwise at the pending process-mem decision gate). Verify never mutates the destination mem's **entities** — that boundary is enforced structurally (`&Engine` shared borrow over the measurement pass). It does write measurement bookkeeping outside that borrow: the findings store, an anchor-hash backfill, and the `#verified` baseline. Reconcile's judgment is absorbed into the engine-rendered sync brief and proven carried by a committed reconcile-absorption diff artifact that gates `/reconcile`'s retirement. The five `ingest@0.1.0` process mems become a second home for the same knowledge — opening the process-mem fate as a genuine operator decision (parked as an open operator decision). Downstream: E3b's verify tiers, coverage/accuracy denominators, and the sync brief's drift slices all read this store; the plugin sync/verify routers and non-code pilots consume the briefs.


**Amendment (2026-08-21) — what verify reports when it could not measure.** A read-only measurement surface has a failure mode its own numbers hide: reporting *green* from an input it could not read. The dashboard is therefore three-valued — `clean` / `drifted` / `inconclusive` — and `inconclusive` is forced, not chosen, by any blind spot the pass detects: a non-enumerable denominator, an enumerated zero, no observed anchors, no readable change signal, or a declared change strategy whose signal the pass could not actually resolve (a declared `git` source with no reachable repository). The capability row keeps reporting the *declaration*; the freshness row reports what the pass could *read*. A verdict may only ever be downgraded by a blind spot, never upgraded, so silence about coverage can never present as coverage.

This makes verify gateable by machines without a second surface. `--fail-on-findings` opts a run into a dedicated exit code (6) for "the mem drifted from its source", constructed at exactly one site and never returned for an operational error — so a CI job can distinguish drift from an engine that could not run, which is the whole point of a gate: both otherwise look like a red build. The findings report reaches stdout *before* the gate fails, under a versioned `memstead-verify/v1` envelope, so a red build always carries the document explaining it. `inconclusive` deliberately has no exit code of its own — a caller that must fail on it reads `rollup.verdict` from that envelope, which keeps the exit-code vocabulary small and the blind-spot policy the caller's to set.

## Relationships
- **REFERENCES**: [[engine:anchor-primitive]]
- **REFERENCES**: [[engine:memstead-base-crate]]

## Options

Considered keeping the refinement writer alongside sync (rejected: two writers, two prose sets, a temp handover that evaporates after 10 minutes — the incoherence the design phase resolved). Considered findings in the `ingest@0.1.0` process mems (rejected for the verify loop: no keyed invalidation, measurement routed through the mutation/commit-provenance surface, no token-budgeted rendering — though what the mems uniquely hold, agent-authored mined-source judgment ledgers, is why the process-mem fate is a real operator decision, not a mechanical migration).

## Notes


