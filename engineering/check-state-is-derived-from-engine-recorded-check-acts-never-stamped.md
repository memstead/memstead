---
type: decision
created_date: 2026-08-08T23:47:36Z
last_modified: 2026-09-27T20:10:54Z
status: accepted
decided_on: 2026-08-09
deciders: operator (agent-trust plan 14, bundle README decision 4)
scope: subsystem
tags: check, verification, derived-state, process-tier, trust-model
---

# Check state is derived from engine-recorded check acts never stamped

## Decision
The engine gains a CHECK OPERATION — the recorded act "entity E checked, verdict ok | failed, via method M" — as the bundle's single deliberate tool addition (`memstead_check` on the MCP server (corrected 2026-09-08: one server since the one-flavour fold of 2026-09-05), `memstead check` on the CLI). A check is engine state, never entity content: it produces no entity write, no mem commit, no `content_hash` change — that non-mutation is load-bearing, because per-entity check state (`never_checked` | `checked_ok` | `check_failed` | `check_stale`) is DERIVED by comparing the check record's entity-hash against the current one, never stamped into a field. Records append to the workspace check ledger (`.memstead/state/checks/checks.jsonl`), append-only with no rotation cap; a newer check supersedes older ones for state derivation but never erases them. Each record carries mutation-provenance identity (actor, client, caller-declared role from [[mutation-provenance-records-caller-declared-roles-tamper-evidently-in-append-only-history]]) plus the entity hash at check time. Recording is never best-effort: a persistence failure refuses typed (`CHECK_NOT_RECORDED`) — a caller who believes an unrecorded check landed is the exact self-report dishonesty this tier removes. The verdict vocabulary is closed (`ok` | `failed`); nuance goes in the method note or process-mem entities, and the operation never auto-creates process entities (deterministic engine, agent judgment).

## Context
Both field projects hand-built and could not keep honest the distinction "checked and sound" vs "never checked" vs "checked, but changed since" — schema fields (`status`, `checked_by`, `parents_at_check`) are self-report, forgettable, and invisible to cross-mem queries. With the mutation-provenance substrate recorded, the process tier derives that distinction instead of declaring it. A check deliberately does NOT ride the anchors/derivations sidecar pattern (same-commit staging) because a check has no accompanying mutation and must not create commits — workspace-local engine state is the only shape that keeps checking-touches-nothing true.

## Consequences
"Checked, but changed since" is now computable and honest: any edit after an ok-check flips the derived state to `check_stale` without anyone remembering to update a field. The state serves in the entity read's opt-in provenance block, so a reasoning loop reads verification state where it reads identity. Costs accepted: the check ledger is workspace-local (it does not travel with the mem-repo the way commit trailers do — acceptable for the gate tier, which runs where the workspace lives); a failed check on since-changed content also reads `check_stale` (the verdict no longer speaks to current content either way). Foreseen consumers: the author≠checker independence gate and the per-mem health axis derive from these records rather than from any self-written field.

## Relationships
- **REFERENCES**: [[mutation-provenance-records-caller-declared-roles-tamper-evidently-in-append-only-history]]
- **REFERENCES**: [[record-semantic-conformance-as-a-kinded-schema-bound-check]]

## Options

Check state as schema fields with engine blessing — rejected: the bundle's core decision; both projects proved the failure mode. Folding the check into `memstead_update` — rejected: a check must never mutate, and overloading a mutation verb with a non-mutation is the response/action polymorphism the surface policy forbids. Riding the mem-branch sidecar (anchors precedent) — rejected: that stages in mutation commits; a standalone check commit would break checking-touches-nothing.

## Notes

Amended 2026-08-27 by [[record-semantic-conformance-as-a-kinded-schema-bound-check]]: checks now carry a closed kind vocabulary (`verification`, the default, and `conformance`), and supersession is per (entity, kind) rather than per entity — a newer check supersedes older ones OF THE SAME KIND for state derivation. A `conformance` record additionally carries the mem's engine-stamped schema pin and derives stale on a pin move as well as a hash move. Everything else in this decision (non-mutation, derived state, closed verdicts, never-best-effort recording) is unchanged and now holds per kind.


2026-09-27 amendment: the ledger gains a transport, not a new home. A project that uses the engine found that a server which pulls a released branch and exports it seals no check records, because the ledger is a workspace file and a check is never a commit. The mem-repo now carries one more engine-owned ref beside the registry ref, `__MEMSTEAD_CHECKS`, a tree with `mems/<mem>/checks.jsonl` per mem holding the mem's ledger rows in a transport form (one JSON line per record, ordered by timestamp then bytes). `push <mem>` publishes the mem's rows on top of the remote's tip and pushes the ref beside the branch, `fetch` and `pull` fetch it and union the mem's rows into the local ledger, `push --all` carries it like the registry ref. A row is a fact whose identity is its bytes: machines merge by set union, nothing is rewritten, a round trip adds nothing, and a race between publishers is a non-fast-forward retried once after importing what landed. The local ref is a vehicle pointed at the remote's tip before every publish, so `status --remote` never classifies it. The check verb stays a non-git act and this decision's non-mutation property holds unchanged. Trust rule: rows arrive only through fetch, pull and push of a mem this workspace mounts, and only that mem's rows; a fork's ref is never imported (both fork forms fetch the branch they name and nothing else), so a contributor cannot ship checker-role rows that close the owner's gates; imported rows keep identity and role and read as local rows do. Rejected: the ledger on the mem branch (every check becomes a mutation: the tip moves, proposal pins go stale, cursors and review marks shift); git notes (keyed by commit, records key by entity and hash); shipping sealed archives instead of branches (works, but not the branch path asked for). Deciders: operator (in-session, 2026-09-27), memstead-e9.
