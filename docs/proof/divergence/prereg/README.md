# Divergence campaign — pre-registration package

This folder is the **pre-registered package** for the divergence eval: everything the campaign consumes, fixed and committed before any arm runs. A reader must be able to verify from this folder alone that nothing was tuned after results existed. The design rationale lives in the plan bundle (`dev/plans/divergence-eval/`); this package is the executable, auditable form of it.

The one measured question: **does write-time schema enforcement produce a measurable advantage over a tolerant markdown substrate as a knowledge base diverges over time?** Two substrates are filled by memoryless writer sessions over ten rounds of a real, evolving source — Arm A tolerant (accept every write), Arm B engine-gated (schema refusals, typed relations) — and read back by scored queries at checkpoints. The pre-registered decision bands make the outcome binding in both directions; a negative result is published exactly as a positive one would be.

## The amendment rule (binding)

**After the first real campaign run, nothing in this package changes except by a dated amendment note appended to this README.** Silent post-run edits are forbidden — they would destroy the pre-registration guarantee that gives the result its credibility. Every amendment states: the date, what changed, and why. Plan 03's published report must surface every amendment note verbatim. Before the first run, the package is still under construction (see Status) and edits are ordinary authoring, not amendments.

### Amendments

**A1 — 2026-07-14 (pre-first-run) — allowance operationalisation.** Discovery during harness work (plan 02, handover 15): `claude -p` offers no output-token cap, so the writer allowance (8,000 full / 4,000 hurry) cannot be enforced as a literal token ceiling. Operator decision (2026-07-14): allowances are enforced as **proportional cost budgets via `--max-budget-usd`** — `budget_usd(round) = allowance_tokens × usd_per_output_token(pinned model)`, the conversion constant recorded in `campaign.json` at implementation time (still pre-first-run). Hurry rounds therefore receive literally half the budget, and an over-budget session is cut off (both arms equally — realistic pressure). **Verification + pre-declared fallback:** plan 03's smoke run must demonstrate the flag actually terminates an over-budget session under the operator's subscription; if it proves inert, the fallback applies without further decision: allowances become documentary targets, hurry pressure rests solely on the terser hurry skeleton, the ledger publishes actual usage per session, and the published report must state that allowances were not hard-enforced.

**A2 — 2026-07-14 (pre-first-run) — round-input rule (what fills `{ROUND_SLICE_CONTENT}`).** A kara slice spans 300–500 commits — more than any session ingests — so "the slice" needed a definition. Operator decision (2026-07-14): the writer input is a **mechanical digest, byte-identical for both arms, with no LLM pre-summarisation**: (a) the `CHANGELOG.md` delta between the slice's boundary commits; (b) the bug-ledger delta (records added or status-changed within the slice); (c) `git log --oneline` commit subjects for the slice range (author-date boundary rule, as pinned in `slices.json`); (d) the slice diffstat (`git diff --stat` between boundaries). Nothing else. Rationale: arm-neutral, bounded, mechanically derivable, and it mirrors what a real maintainer reads when catching up; the query battery's ground truth derives from the same public sources (ledger / changelog / roadmap), so the digest carries the information the battery tests without pre-answering it — reference answers remain derived from the full pinned snapshot, never from the digest.

## Files

| File | What it is | Status |
|---|---|---|
| `source-repo.md` | The source repository record: `karalang/kara`, pinned HEAD, per-criterion selection evidence, rejected alternatives. | **Committed** |
| `slices.json` | The slice manifest: ten SHA-pinned chronological slices, the boundary rule, and the structural events (rename / revert / supersession) named per slice. | **Committed** |
| `models.json` | The model pins, captured from the authoring session and frozen. | **Committed** |
| `campaign.json` | Machine-readable campaign parameters (schedule, hurry rounds, trials, allowances, cost cap, contamination threshold, band thresholds) — the harness's config interface; the machine encoding of `bands.md`, which stays the human exposition. | **Committed** |
| `arms.md` | Arm A / Arm B definitions, the Arm-A toolset justification, and the two writer + two reader prompts (shared skeleton + substrate block; diff only in substrate/access mechanics). | **Committed** |
| `prompts.json` | Machine-readable form of the prompts — the `arms.md` writer/reader skeletons + per-arm substrate blocks (`{SUBSTRATE_BLOCK}`/`{ROUND_SLICE_CONTENT}`/`{QUERY}`), plus the arm-neutral `auditor_skeleton` (`{CORPUS}`) that machine-encodes `rubrics.md`'s integrity counting rubric; the harness's prompt interface (the markdown stays the human exposition; verified to match it verbatim). | **Committed** |
| `tell-lists.json` | Both arms' judge tell lists (Arm B mem/tool vocabulary; Arm A substrate vocabulary), for two-directional blinding — every active tell verified substring-disjoint from the source. | **Committed** |
| `queries.json` | The twelve-query battery: per-query ground truth (source citations only) + tell-free reference answers. | **Committed** |
| `rubrics.md` | The judge scoring rubric (incl. list-answer F1 partial credit) and the integrity counting rubric, with the verbatim arm-neutral auditor prompt that operationalises it. | **Committed** |
| `bands.md` | The decision bands (uncertainty + slope qualifiers), the integrity margin, the checkpoint schedule, trial counts, token allowances, hurry-mode rounds, the cost cap, and the combined verdict table. | **Committed** |

## Status

**Complete — pre-first-run, no campaign has run.** Every artifact the campaign consumes is committed: the source repository and its pinned slices, the frozen model pin, the query battery with mechanically-derived ground truth, both tell lists, the arm definitions and prompts, the scoring rubrics, and the decision bands with the combined verdict table. Reading this folder alone suffices to re-run or audit the campaign design. Nothing here changes now except by a dated amendment note.
