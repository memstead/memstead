# Divergence campaign — pre-registration package

This folder is the **pre-registered package** for the divergence eval: everything the campaign consumes, fixed and committed before any arm runs. A reader must be able to verify from this folder alone that nothing was tuned after results existed. The design rationale lives in the plan bundle (`dev/plans/divergence-eval/`); this package is the executable, auditable form of it.

The one measured question: **does write-time schema enforcement produce a measurable advantage over a tolerant markdown substrate as a knowledge base diverges over time?** Two substrates are filled by memoryless writer sessions over ten rounds of a real, evolving source — Arm A tolerant (accept every write), Arm B engine-gated (schema refusals, typed relations) — and read back by scored queries at checkpoints. The pre-registered decision bands make the outcome binding in both directions; a negative result is published exactly as a positive one would be.

## The amendment rule (binding)

**After the first real campaign run, nothing in this package changes except by a dated amendment note appended to this README.** Silent post-run edits are forbidden — they would destroy the pre-registration guarantee that gives the result its credibility. Every amendment states: the date, what changed, and why. Plan 03's published report must surface every amendment note verbatim. Before the first run, the package is still under construction (see Status) and edits are ordinary authoring, not amendments.

### Amendments

_(none — no campaign run has occurred)_

## Files

| File | What it is | Status |
|---|---|---|
| `source-repo.md` | The source repository record: `karalang/kara`, pinned HEAD, per-criterion selection evidence, rejected alternatives. | **Committed** |
| `slices.json` | The slice manifest: ten SHA-pinned chronological slices, the boundary rule, and the structural events (rename / revert / supersession) named per slice. | **Committed** |
| `models.json` | The model pins, captured from the authoring session and frozen. | **Committed** |
| `arms.md` | Arm A / Arm B definitions, the Arm-A toolset justification, and the two writer + two reader prompts (shared skeleton + substrate block; diff only in substrate/access mechanics). | **Committed** |
| `tell-lists.json` | Both arms' judge tell lists (Arm B mem/tool vocabulary; Arm A substrate vocabulary), for two-directional blinding — every active tell verified substring-disjoint from the source. | **Committed** |
| `queries.json` | The twelve-query battery: per-query ground truth (source citations only) + tell-free reference answers. | **Committed** |
| `rubrics.md` | The judge scoring rubric (incl. list-answer F1 partial credit) and the integrity counting rubric. | **Committed** |
| `bands.md` | The decision bands (uncertainty + slope qualifiers), the integrity margin, the checkpoint schedule, trial counts, token allowances, hurry-mode rounds, the cost cap, and the combined verdict table. | **Committed** |

## Status

**Complete — pre-first-run, no campaign has run.** Every artifact the campaign consumes is committed: the source repository and its pinned slices, the frozen model pin, the query battery with mechanically-derived ground truth, both tell lists, the arm definitions and prompts, the scoring rubrics, and the decision bands with the combined verdict table. Reading this folder alone suffices to re-run or audit the campaign design. Nothing here changes now except by a dated amendment note.
