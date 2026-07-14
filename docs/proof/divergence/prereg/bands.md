# Decision bands, schedule, and cost

Pre-registered. Immutable after the first real run except by a dated amendment note (see [README.md](README.md)). Every value here is a single unambiguous number or a fixed rule — nothing is left to judgment during the run. The bands are applied **mechanically** in plan 03 from the harness's reported deltas and standard errors.

## Fixed campaign parameters

| Parameter | Value |
|---|---|
| Rounds | 10 |
| Hurry rounds (half allowance, terse prompt) | 3, 6, 9 |
| Reader checkpoints (after round) | 1, 3, 5, 10 |
| Integrity audit (after round) | 5 (trend), 10 (primary) |
| Trials per query per arm (`n`) | 3 |
| Trials per integrity audit (`n`) | 3 |
| Writer token allowance — full round | 8,000 tokens |
| Writer token allowance — hurry round | 4,000 tokens |
| Reader token budget (per answer) | 8,000 tokens |
| Contamination screen threshold (bare model) | 0.5 |
| Models (writer / reader / judge / auditor) | as pinned in [models.json](models.json) — `claude-opus-4-8`, passed explicitly to every runner |
| Hard cost cap (total) | 20,000,000 tokens |

**Cost cap enforcement.** The cost ledger sums every writer, reader, judge, and auditor token across both arms, including Arm B's refusal-repair loops. If the running total reaches 20,000,000 tokens the campaign **aborts and hands over** — never a silent scope cut. The ledger is published as-is regardless of outcome.

## Accuracy band — round-10 delta `B − A`

Let `d10` be the round-10 accuracy delta (`B − A`, [rubrics.md](rubrics.md)), `se10` its standard error across trials, and `d1` the round-1 delta.

- **positive** requires **all three**: `d10 ≥ +0.10` **and** `d10 − se10 > 0` **and** the divergence contribution `d10 − d1 > 0`.
- **static surface advantage, no divergence** (a distinct named outcome, never counted as positive): `d10 ≥ +0.10` is met but the divergence contribution `d10 − d1 ≤ 0` — i.e. the round-10 gap was already present at round 1 and did not grow. This is a static read-surface advantage, not divergence over time.
- **negative**: `d10 ≤ −0.05` **and** `d10 + se10 < 0`.
- **null**: any result meeting none of the above.

The `d10 − se10 > 0` (and `d10 + se10 < 0`) conditions are the uncertainty guard: a point estimate alone never clears a band. The `d10 − d1 > 0` condition is the slope qualifier that keeps a static access-surface advantage out of the positive verdict.

## Integrity band — round-10 delta `A − B` (defects per 100 items)

Let `g10` be the round-10 integrity delta (`A − B`, defects per 100 items, [rubrics.md](rubrics.md)) and `se10g` its standard error across trials.

- **positive** (enforcement kept the corpus measurably cleaner): `g10 ≥ +1.0` **and** `g10 − se10g > 0`.
- **negative** (enforcement left the corpus dirtier): `g10 ≤ −1.0` **and** `g10 + se10g < 0`.
- **null**: neither.

The `±1.0 defects per 100 items` margin is the minimum effect size counted as measurable; the round-5 audit is reported as the integrity trend but does not move the band.

## Combined verdict table (binds the report language)

Every reachable combination of the two band outcomes maps to exactly one bound verdict. The accuracy column collapses `null` and `static surface advantage` where they share a verdict, but they remain distinct outcomes and the report must name which one occurred.

| Accuracy | Integrity | Bound verdict |
|---|---|---|
| positive | positive | Enforcement pays end-to-end: the schema-gated substrate both answers better as it diverges and stays measurably cleaner. |
| null **or** static-advantage | positive | The substrate diverges (Arm B stays cleaner) but the read surface does not yet capitalise it into better answers — a read-side product finding, **not** a falsification of the write gate. |
| positive | null | A read-surface advantage without measured substrate divergence — attributed to the access surface, not the gate. |
| null **or** static-advantage | null | No divergence demonstrated at this scale and round count — the value thesis fails this test. |
| negative (either endpoint) | any | Reported as a loss on that endpoint, verbatim, whatever the other endpoint shows. |

**Reachability.** The two bands each take one of {positive, negative, null}, and accuracy additionally distinguishes static-advantage; the negative row absorbs any combination containing a negative on either endpoint, so every one of the remaining positive/null/static combinations is assigned by one of the first four rows. No reachable combination is left without a verdict.

## Allowance operationalisation

The token allowances above are enforced as proportional cost budgets via `claude -p --max-budget-usd` (conversion constant in `campaign.json`), with a smoke-run verification and a pre-declared documentary fallback — the binding definition is amendment **A1** in `README.md`.
