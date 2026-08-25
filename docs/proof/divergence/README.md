# Divergence campaign: does write-time enforcement pay as a knowledge base diverges? (2026-07)

A pre-registered, ten-round controlled measurement. Everything the campaign
consumes was committed before a single arm ran (`prereg/`), the decision bands
were fixed in advance and are applied mechanically below, and both endpoints are
published exactly as they came out.

> **The one measured question:** does write-time schema enforcement produce a
> measurable advantage over a tolerant markdown substrate as a knowledge base
> diverges over time?

The answer is split, and the split is the finding. **The engine-gated arm kept
its corpus free of duplicates and contradictions where the tolerant arm drifted;
it also answered questions worse, and the answer gap widened over the campaign.**
One endpoint banded positive, the other negative. Both are reported.

## The two arms

Two substrates were filled by **memoryless writer sessions** over ten rounds of a
real, evolving source repository. Each round both arms received the identical
mechanical slice digest (changelog delta, bug-ledger delta, `git log --oneline`
subjects, diffstat, with no LLM pre-summarisation).

- **Arm A, the tolerant markdown directory.** Free-string `type:` in YAML
  frontmatter, `[[wikilinks]]`, an optional index. Every write accepted as-is, no
  validation, no typed vocabulary. Read and written with filesystem tools.
- **Arm B, the engine-gated mem.** Pinned to `software@0.1.0`. Every write validated
  at write time; a violating write is refused with a recovery payload the writer
  repairs from. Read with the engine's read tools.

Writer prompts were byte-identical apart from one delimited substrate block
containing only access mechanics. Writers were blind to being measured. Readers
were instructed to answer directly and never describe how or where they found an
answer, and every answer was tell-stripped before reaching a blinded judge.
Rounds 3, 6 and 9 ran at half allowance with a terser prompt (deliberate time
pressure).

## Result 1: accuracy (co-primary), band **NEGATIVE**

Twelve-query battery, 3 trials per query per arm, blinded judge. Delta is
`B − A`; the standard error is the harness's paired statistic (population
standard deviation of the twelve per-query deltas, divided by `sqrt(12)`).

| Checkpoint | Arm A (tolerant) | Arm B (engine) | `d` (B−A) | `se` |
|---|---:|---:|---:|---:|
| after round 1 | 0.157 | 0.113 | −0.044 | 0.050 |
| after round 3 | 0.147 | 0.094 | −0.053 | 0.077 |
| after round 5 | 0.317 | 0.246 | −0.071 | 0.084 |
| **after round 10** | **0.575** | **0.286** | **−0.289** | **0.120** |

Applying [`prereg/bands.md`](prereg/bands.md) mechanically at round 10:

- `positive` requires `d10 ≥ +0.10` **and** `d10 − se10 > 0` **and** `d10 − d1 > 0`. All three fail.
- `negative` requires `d10 ≤ −0.05` **and** `d10 + se10 < 0`. Both hold: −0.289 ≤ −0.05, and −0.289 + 0.120 = **−0.169 < 0**.

**Band: negative.** The slope is negative too (`d10 − d1 = −0.244`): the gap was
not a static surface difference present from round 1, it grew monotonically
across all four checkpoints.

## What the accuracy endpoint substantially measures (added 2026-08-25)

A direct count over the two committed corpora, made four weeks after the run,
changes how the accuracy number should be read. It is recorded here rather than
left for a reader to discover.

| | Arm A (tolerant) | Arm B (engine) |
|---|---:|---:|
| distinct bug identifiers | **244** | **26** |
| files mentioning bugs | 26 | 10 |
| entities of the schema's `incident` type | n/a | **0** |

Arm A holds a dedicated `bug-tracker.md`. Arm B holds no operational records at
all: the pinned `software@0.1.0` schema declares an `incident` type and the
writers used it zero times in ten rounds.

**Five of the twelve queries ask about bug records** (`S1-open-bugs`,
`S2-open-high-sev`, `A1-codegen-bug-count`, `A2-ledger-totals`,
`A3-open-codegen-bugs`), so 42 percent of the accuracy endpoint asks one arm for
material it largely does not hold. Under the pre-registered F1 rubric an honest
"the knowledge base does not contain this" scores exactly 0.0, the same as a
wholly wrong answer, because an empty item set has neither precision nor recall.

A follow-up experiment put `S1-open-bugs` to four independent access surfaces
over Arm B's corpus, including one holding the entire corpus in context with no
tools. All four answered that the material is not there; the engine surface named
the zero-instance `incident` type itself. A surface holding everything cannot
fail at retrieval, so the absence is in the bytes.

The consequence is a relocation, not a rescue. **A substantial part of the
accuracy loss is a gap in what Arm B's writers recorded, not in how its corpus
reads.** This cuts against the engine rather than for it, and it raises a
question the campaign never asked: under the same prompts, the same source
digests and fewer writer tokens (67M against Arm A's 158M), the schema-gated
writers produced more items (81 against 41) and almost none of the operational
material. Whether the type vocabulary steered them or the per-entity write cost
made them economise is untested, and both are testable by counting.

## Result 2: integrity (co-primary), band **POSITIVE**

Three blinded auditor sessions per arm read the tell-stripped corpus and counted
duplicates and contradictions, normalised to defects per 100 items. Delta is
`A − B`, so a positive number means Arm A is dirtier.

| Checkpoint | Arm A items | Arm B items | A defects/100 | B defects/100 | `g` (A−B) | `se` |
|---|---:|---:|---:|---:|---:|---:|
| after round 5 (trend) | 32 | 66 | 0.00 | 1.52 | −1.52 | 0.71 |
| **after round 10 (primary)** | **41** | **81** | **8.13** | **0.00** | **+8.13** | **2.39** |

At round 10 the engine-gated corpus contained **zero** auditor-detected
duplicates or contradictions across all three independent audits (per-trial: 0.0,
0.0, 0.0), while the tolerant corpus scored 12.20, 9.76 and 2.44 per 100 items.
Arm B held twice the item count while doing it.

`positive` requires `g10 ≥ +1.0` **and** `g10 − se10g > 0`. Both hold: 8.13 ≥ 1.0,
and 8.13 − 2.39 = **+5.74 > 0**. **Band: positive.**

**The round-5 trend ran the other way and is reported as it stands.** At the
mid-campaign audit Arm B was the dirtier corpus (1.52 against 0.00). Per the
pre-registration the round-5 audit is the trend and does not move the band, but a
reader should know the ordering reversed between the two audits, and that each
audit rests on three trials.

## The bound verdict

The pre-registered verdict table assigns exactly one reading to every reachable
combination. With accuracy negative, one row applies, verbatim:

> | Accuracy | Integrity | Bound verdict |
> |---|---|---|
> | negative (either endpoint) | any | Reported as a loss on that endpoint, verbatim, whatever the other endpoint shows. |

So: **a measured loss on the accuracy endpoint, and a measured win on the
integrity endpoint.** The campaign does not license the sentence "enforcement
pays end-to-end", and it does not license "enforcement does not work" either. It
licenses exactly the two numbers above.

The nearest non-negative row of the same table names the shape this result has,
and it is a fair description of what happened even though the negative row is the
one that binds:

> The substrate diverges (Arm B stays cleaner) but the read surface does not yet
> capitalise it into better answers — a read-side product finding, **not** a
> falsification of the write gate.

## Secondary signal: vocabulary entropy (judge-free)

Computed from substrate bytes after each round's writers ran, with no model in
the path.

| Round | A distinct types | A distinct relation labels | B distinct types | B distinct relation labels |
|---:|---:|---:|---:|---:|
| 1 | 9 | 35 | 7 | 43 |
| 5 | 9 | 114 | 7 | 106 |
| 10 | 9 | **219** | 7 | **159** |

Arm A's free-string type field stayed at nine values, but its relationship
vocabulary sprawled to 219 distinct labels. Arm B's types are schema-bounded at
seven and its relation labels grew to 159. Read this as a rough divergence
signal, not as a scored endpoint: the two arms count edges differently and the
measure was pre-registered as secondary.

## Cost

The full ledger is published regardless of outcome, per the pre-registration.

| | Tokens |
|---|---:|
| Total (all roles, both arms) | 293,044,152 |
| Non-cache (the accounting the cost brake counts) | 1,628,617 |
| Cap | 40,000,000 |
| Arm A writer / reader | 158,302,964 / 17,343,347 |
| Arm B writer / reader | 67,392,168 / 41,849,457 |
| Judge / auditor | 6,482,674 / 1,673,542 |

## Honest bounds, read before citing

- **One campaign, one source repository, one model.** Ten rounds, twelve queries,
  three trials, two integrity audits of three trials each. Not a benchmark, and
  no variance estimate exists across campaigns.
- **Read surface and write gate are inseparable here, and it was pre-declared.**
  Each arm was read with its substrate's native access surface: filesystem tools
  for a markdown directory, the engine's read tools for a mem. The
  pre-registration states plainly that "tool-surface differences are part of the
  measured variable, not a confound to be denied." The accuracy loss therefore
  cannot be attributed to typing rather than to the engine's read tools, in
  either direction. **The cell that would separate them, typed files read with
  filesystem tools, was not run.**
- **The integrity result has no identified mechanism.** The engine ships no
  duplicate detector and no contradiction detector. Zero defects across three
  audits is a measured outcome of the write gate plus the typed vocabulary, and
  the causal path is a hypothesis, not a feature. A cheap discriminating test
  exists and has not been run: a third arm of plain markdown plus a thin
  vocabulary linter, no engine. If the linter also reaches zero, the value is the
  gate idea rather than this engine.
- **The round-5 integrity ordering was reversed** (see above). Two audits, three
  trials each, is a thin basis for a claim about a trajectory.
- **Both arms score low in absolute terms.** At round 10 the better arm answered
  the battery at 0.575. Neither substrate produced a corpus that answers this
  battery well, and the delta is a comparison between two mediocre results.
- **No arm was ever verified to be the arm it claims.** The harness recorded no
  per-session check that Arm B's reader actually reached the engine. A follow-up
  build over the same apparatus found exactly that failure in its own engine arm:
  a misconfigured MCP server left the session with no engine tools at all, it
  fell back to file tools, and it returned a plausible mid-range score that would
  have entered a result table unchallenged. Only the tool-call list revealed it.
  Nothing here shows the campaign suffered that failure; nothing here shows it
  did not, because the evidence was never recorded. A per-session guard costs
  about ten lines and is now standard in this project's read-side harness.
- **Allowances were not hard-enforced.** Amendment A3 moved the writer token
  allowance to a documentary target after the calibrated budget proved
  unworkable. Both arms ran uncapped and equally; actual usage is published in
  the ledger above. Arm B's structurally higher write cost is measured and
  published, not hidden.

## Amendments (verbatim, per the package's amendment rule)

The pre-registration binds every change to the package after the first run to a
dated amendment note, and binds the published report to surfacing every note
verbatim. A1 to A4 predate the first run; A5 is post-run and non-experimental.

They are quoted from [`prereg/README.md`](prereg/README.md), so their internal
references ("this README", "the Status section below") point at that file, not
at this one.

**A1 — 2026-07-14 (pre-first-run) — allowance operationalisation.** Discovery during harness work (plan 02, handover 15): `claude -p` offers no output-token cap, so the writer allowance (8,000 full / 4,000 hurry) cannot be enforced as a literal token ceiling. Operator decision (2026-07-14): allowances are enforced as **proportional cost budgets via `--max-budget-usd`** — `budget_usd(round) = allowance_tokens × usd_per_output_token(pinned model)`, the conversion constant recorded in `campaign.json` at implementation time (still pre-first-run). Hurry rounds therefore receive literally half the budget, and an over-budget session is cut off (both arms equally — realistic pressure). **Verification + pre-declared fallback:** plan 03's smoke run must demonstrate the flag actually terminates an over-budget session under the operator's subscription; if it proves inert, the fallback applies without further decision: allowances become documentary targets, hurry pressure rests solely on the terser hurry skeleton, the ledger publishes actual usage per session, and the published report must state that allowances were not hard-enforced.

**A2 — 2026-07-14 (pre-first-run) — round-input rule (what fills `{ROUND_SLICE_CONTENT}`).** A kara slice spans 300–500 commits — more than any session ingests — so "the slice" needed a definition. Operator decision (2026-07-14): the writer input is a **mechanical digest, byte-identical for both arms, with no LLM pre-summarisation**: (a) the `CHANGELOG.md` delta between the slice's boundary commits; (b) the bug-ledger delta (records added or status-changed within the slice); (c) `git log --oneline` commit subjects for the slice range (author-date boundary rule, as pinned in `slices.json`); (d) the slice diffstat (`git diff --stat` between boundaries). Nothing else. Rationale: arm-neutral, bounded, mechanically derivable, and it mirrors what a real maintainer reads when catching up; the query battery's ground truth derives from the same public sources (ledger / changelog / roadmap), so the digest carries the information the battery tests without pre-answering it — reference answers remain derived from the full pinned snapshot, never from the digest.

**A3 — 2026-07-14 (pre-first-run) — allowance enforcement moves to A1's documentary fallback, with an honestly extended trigger.** The live smoke run verified `--max-budget-usd` is NOT inert under the operator's subscription — it genuinely cuts over-budget sessions — but the A1-calibrated budget ($0.20/round from 8,000 tokens × list price) proved unworkable at real slice sizes: it cut the Arm B writer before a single `memstead_*` mutation completed on the round-1 digest, invalidating the round. A1's fallback was declared for "flag proves inert"; operator decision (2026-07-14) extends the trigger to "flag works but the calibrated level is unworkable" and adopts the fallback as declared: campaign runs with `--no-budget`; the allowance numbers are documentary targets, not enforced caps; hurry-round pressure rests solely on the terser hurry skeleton; the ledger publishes actual per-session usage; and the published report must state that allowances were not hard-enforced. Arm parity is unaffected (same model, same prompts, both arms uncapped); Arm B's structurally higher write cost is measured and published, not hidden — the cost-adjusted secondary reading exists for exactly this.

**A4 — 2026-07-14 (pre-first-run) — cost-cap accounting counts non-cache tokens; threshold raised to 40M.** The smoke run showed the raw-token cap misfires as a cost brake: one reduced round consumed ~7M raw tokens, writers ~6.4M of it cache-read-dominated — and cache reads cost roughly a tenth of fresh tokens, so a raw-count cap fires ~10× too early relative to real cost and would abort the full campaign within ~2 rounds (partial results, no verdict). Operator decision (2026-07-14): the abort threshold counts **non-cache tokens only** (fresh input + output); the threshold is **40,000,000 non-cache tokens**, sized with headroom from the smoke extrapolation (the run session records the exact smoke-derived projection alongside the machine value in `campaign.json`). Transparency is unchanged: the ledger continues to publish all raw totals including the cache split; only what the brake counts changes. Shrinking the round digest was rejected — it would alter the experiment itself rather than fix the brake's units.

**A5 — 2026-07-20 (post-run) — leak repair and a stale Status correction. No experimental content changed.** Two non-experimental defects were found while running the public repo's `scripts/leak-scan.sh`, which this package had never passed. (a) This README's opening paragraph pointed the reader at a planning-bundle path that exists only in the project's private workspace repo — a public artifact deferring to a document no reader can open. The pointer is removed; the sentence now states that the rationale is private and that this package is self-contained. (b) `../arm-b-mem/capture.mcp.json` embedded the operator's absolute home path (`/Users/…`) twice; it now uses repo-root-relative paths, so the recorded launch command is machine-independent and actually re-runnable by a reader. (c) The **Status** section still read "pre-first-run, no campaign has run" although `../state.json` records `completed_rounds: 10` and `../result.json` holds the checkpoint scores — the campaign ran in full. Status is corrected below to describe the package as frozen-and-consumed. No pre-registered parameter, prompt, band, query, rubric, slice, or model pin was touched, and no result was recomputed; the scored artifacts are byte-unchanged. Recorded here because the amendment rule binds every change to this package after the first run, including changes that are not experimental.

## Files

| File | What it is |
|---|---|
| `prereg/` | The pre-registration package: source repo record, SHA-pinned slices, model pins, campaign parameters, arm definitions and prompts, tell lists, the twelve-query battery with ground truth, scoring rubrics, and the decision bands. Self-contained; readable without this report. |
| `result.json` | The harness output: per-checkpoint per-query scores, the integrity checkpoints, the entropy series, and the cost ledger. Every number in this report is read from it. |
| `state.json` | Campaign progress (`completed_rounds: 10`) and the resume state. |
| `arm-a/` | Arm A's round-10 corpus, 41 markdown files. |
| `arm-b-mem/` | Arm B's round-10 corpus, 82 files. |
| `README.md` | This report. |

## Reproducing the bands

Every band above is recomputed from `result.json` alone, with the harness's own
aggregation (`xtask/src/eval/series.rs`): the accuracy delta is the mean of the
per-query `on_mean` minus the mean of the per-query `off_mean`, and its standard
error is the population standard deviation of the twelve per-query deltas divided
by the square root of twelve. The integrity standard error is the population
standard deviation across the three per-arm trials, combined as
`sqrt(sd_A²/3 + sd_B²/3)`. No number in this report needs the harness to be
re-run.
