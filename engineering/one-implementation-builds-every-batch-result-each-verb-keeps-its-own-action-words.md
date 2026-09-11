---
type: decision
created_date: 2026-09-11T08:18:17Z
last_modified: 2026-09-11T08:18:17Z
status: accepted
decided_on: 2026-09-11
deciders: one-provenance bundle plan 03 (agent, under the standing engine-change rule)
scope: subsystem
tags: write-gate, batch, create, update, relate, kernel
---

# One implementation builds every batch result; each verb keeps its own action words

## Decision
We will build the four results a batch verb ends in once, in the mutation module: the refusal that names every failing entry up to `BATCH_ERROR_REPORT_CAP` detailed envelopes with `errors_suppressed` counting the rest and every valid entry marked `not_applied`; the rehearsal and applied receipts, one entry per item with the action word the verb chose; and the empty-batch result. Batch create, batch update and batch relate call the builders and pass their own words (`created`; `updated` and `noop`; relate's per-item labels and `noop`), and relate alone passes the orphan stubs it collected. The per-item error envelope moves with them. This resolves the second divergence the phase-split decision recorded as preserved ([[engineering--the-write-gate-is-cut-by-phase-resolve-validate-compose-stage-commit-outcome]]).

## Context
After the phase cut the refusal builder, the receipt builder and the empty-batch early return existed in create's outcome phase, in update's outcome phase and inline in relate: three copies of the report-all cap, a gate that must behave identically on every verb, each free to drift. The verbs' action vocabularies differ (relate's labels come from its item state), which is why the cut left the copies rather than force a shared action enum. The principle that a guard on one write path exists on all of them with one implementation ([[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]) asks for the fold; the shape chosen keeps the words with the verbs and the mechanics in one place.

## Consequences
- The cap, the suppressed count and the `not_applied` marking are one comparison in the tree; a refused batch of sixty failing entries reports fifty envelopes and `errors_suppressed: 10` on every verb because there is one place that counts.
- `BatchResult` and `BatchEntry` and every action word are unchanged on the wire; the engine's and the CLI's batch tests are byte-unchanged and green, and the golden of the fixture mutation sequence is byte-identical.
- A fourth batch verb declares its words and calls the builders; it cannot cap differently.

## Relationships
- **REFERENCES**: [[the-write-gate-is-cut-by-phase-resolve-validate-compose-stage-commit-outcome]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

- A generic builder over an action enum shared by the three verbs: rejected, the vocabularies differ and a shared enum would move the verbs' words into one place they do not belong.
- Leave the three copies since they are small: rejected, the report-all cap is a gate and a gate exists once.
- Fold the receipts but keep each verb's refusal: rejected, the refusal is the copy that carries the cap.
