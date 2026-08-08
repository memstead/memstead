---
type: decision
created_date: 2026-08-08T18:40:39Z
last_modified: 2026-08-08T18:40:39Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust plan 07)
scope: subsystem
tags: write-surface, dry-run, rehearsal, batch, mcp, cli
---

# The whole write surface rehearses via dry-run reversing the batch no-dry-run contract

## Decision
Rehearsal (`dry_run`) is a property of the WHOLE write surface, not of individual verbs: `memstead_relate` (MCP list form and CLI) and the batch family (`batch-create` / `batch-update` / `batch-relate`, engine-level batch parameter, CLI `--dry-run`) now honour it, deliberately REVERSING the recorded "the batch family has no dry-run" contract. The rehearsal contract has four legs, all enforced by tests rather than convention: (1) identical validation — the rehearsed call runs the same prepare/validation stage as the real call (shared code, never a copy), so a rehearsed refusal carries the identical typed `{code, message, details}` envelope and a rehearsed accept is followed by a succeeding real call on an unchanged mem; (2) observable zero side effects — git refs, working tree, and `.memstead/` state are byte-identical before and after any rehearsed call (asserted by a recursive tree digest in `memstead-cli/tests/rehearsal_contract.rs`, not assumed); would-be auto-stubs are reported via the normal `AUTO_STUB_CREATED` warning, never created; (3) one marker form — empty `commit_sha` plus the prospective fields the pre-existing create/update dry-run already used; no second marker vocabulary, no response-shape polymorphism; (4) no silent ignoring — the filesystem MCP flavour keeps its typed `UNSUPPORTED_PARAM` refusal (extended to relate) instead of implementing or dropping the flag.

## Context
Agents learn an unfamiliar schema by being refused, and for multi-entity work they need to rehearse the whole plan against the real validator before entity one lands. The engine already believed this for single create/update ([[engine--create-mutation]], [[engine--update-mutation]]), but [[engine--relate-mutation]] had no dry-run at all and the batch family ([[engine--batch-update-atomic-mutation]]) deliberately refused it — a contract set when batching was pure bulk-ingest tooling. Pre-validating a 50-entity build is precisely batch-shaped: intra-batch reference resolution and in-order relate semantics only exist there, so the one surface that refused rehearsal was the one that needed it most. Honouring the old contract against the surface's primary consumers would have been policy outliving purpose. The pre-existing single-verb dry-run had also never been asserted AGAINST the rehearsal contract (side-effect-freeness was implied by design, untested); this decision makes the contract test-enforced for old and new surfaces alike, per [[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]] — rehearsal rides the same shared prepare stage as the real call by construction.

## Consequences
An agent can now pre-validate any write — single or batch — and trust three things: the refusal it sees rehearsed is the refusal it would get for real; a clean rehearsal leaves zero trace anywhere (verified at the byte level, quarantined and read-only mems refuse identically to real calls); and the empty-`commit_sha` marker distinguishes every rehearsed response. Costs accepted: the batch contract reversal is a breaking change to the recorded family contract (pre-1.0, deliberate, changelog-recorded, no surviving old-contract documentation); the MCP multi-op rehearsal response reports the CURRENT source hash per entry (the rolled-back store serves it) where the single-op path reports the prospective hash — both are valid next-`expected_hash` values, and the divergence is documented at the reconstruction site. Engine batch functions gained a batch-level `dry_run` parameter (per-entry `dry_run` inside a batch stays forced off, so batch-level rehearsal is the only preview channel there).

## Relationships
- **REFERENCES**: [[engine:create-mutation]]
- **REFERENCES**: [[engine:update-mutation]]
- **REFERENCES**: [[engine:relate-mutation]]
- **REFERENCES**: [[engine:batch-update-atomic-mutation]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

A separate `memstead_validate` tool — rejected: tool-count policy, and a separate tool invites drift from the real verbs; the flag on the verb IS the shared-code guarantee. Keeping the batch no-dry-run contract — rejected as policy outliving purpose (this decision is the recorded reversal). Implementing dry-run on the filesystem flavour — rejected: it is the deliberately lean surface; a typed refusal is honest and cheap. Rehearse-then-commit tokens (validate once, apply later without re-validating) — rejected: the mem can change between calls; re-validation on the real call is the only honest semantics.

## Notes


