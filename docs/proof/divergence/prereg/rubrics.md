# Scoring rubrics

Pre-registered. Immutable after the first real run except by a dated amendment note (see [README.md](README.md)). No scoring freedom remains for the run: both rubrics below are mechanical given the answers and corpora.

## Judge scoring rubric (accuracy endpoint)

The judge is a fresh agent given a single query's **reference answer** ([queries.json](queries.json)) and a candidate answer with its arm identity removed and both tell lists stripped ([tell-lists.json](tell-lists.json)). It never sees which arm produced the answer, never sees the other arm's answer, and scores each answer independently on `[0, 1]`. Each query is answered at `n = 3` trials per arm; the query's per-arm score is the mean of its three trial scores.

Two query shapes, fixed per query by the `class` and the shape of its ground truth in [queries.json](queries.json):

### Scalar answers — `S2`, `T3`, `C1`, `C2`, `C3`, `A1`, `A2`

A single correct state, count, or explanation. The judge assigns:

- **1.0** — every key fact in the reference answer is present and correct, and no stated fact contradicts the reference.
- **0.5** — the central fact is correct but a secondary key fact is missing or wrong (e.g. `C1`: names the JIT/LLJIT backend correctly but omits that the interpreter is retained as a dev tool; `A2`: correct total but wrong open/fixed split).
- **0.0** — the central fact is wrong, absent, or contradicted (e.g. `A1`: a count off by more than ±5% of 321; `T3`: answers "yes").

The key facts of each reference answer are its explicitly enumerated ground-truth fields (`answer`, `answer_count`, `derivation` highlights); the judge is instructed to treat those as the checklist, not the prose phrasing.

### List answers — `S1`, `S3`, `T1`, `T2`, `A3`

The ground truth is a **set** (bug ids, phase numbers). Partial credit is the **F1 score** of the answer's item set against the ground-truth set:

```
precision = |correct items the answer lists| / |items the answer lists|
recall    = |correct items the answer lists| / |ground-truth items|
score     = F1 = 2 · precision · recall / (precision + recall)     (0 if precision+recall = 0)
```

Item identity is exact on the canonical token (the bug id such as `B-2026-07-14-15`, or the phase label such as `7.1` / `LLJIT-productionization`). An item the answer lists that is not in the ground truth lowers precision; a ground-truth item the answer omits lowers recall. Duplicated items in the answer are de-duplicated before counting. This makes both over-listing (dumping every bug) and under-listing (naming one) score below a precise answer, and it is fully determined by the two sets — the judge does no holistic weighting for list queries.

### Aggregate

Per arm, the accuracy score at a checkpoint is the mean of the twelve per-query scores (each already a 3-trial mean). The signed **accuracy delta is B − A**, carried with its standard error across trials, unfloored and unrounded, exactly as the existing harness reports substrate deltas. The contamination screen (bare-model threshold 0.5, inherited unchanged from the 2026-07-08 substrate eval) runs first; any query the no-substrate bare model already answers at ≥ 0.5 is excluded from the delta and the exclusion is published.

## Integrity counting rubric (integrity endpoint)

A blinded **auditor** agent receives one arm's whole corpus with all arm identity and both tell lists stripped — for Arm A the concatenated markdown bodies, for Arm B the projected entity texts — and counts two defect kinds under the fixed definitions below. The auditor is not told which arm it is auditing, sees only one corpus at a time, and runs at `n = 3` trials with the same mean-and-standard-error treatment as the accuracy endpoint. The audit runs after round 10 (primary) and after round 5 (trend).

### What counts as a duplicate

A **duplicate** is a pair of distinct corpus items (two files in Arm A, two entities in Arm B) whose **primary subject is the same** — the same bug id, the same phase, or the same named design decision — **and** whose factual content substantially overlaps, such that one item is redundant given the other. Each redundant item beyond the first for a given subject counts as **one duplicate**. Restating the same fact inside a single item is not a duplicate; two items about genuinely different aspects of one subject (e.g. a phase's *status* vs its *dependency list*) are not duplicates.

### What counts as a contradiction

A **contradiction** is a pair of corpus items that assert **incompatible facts about the same subject**: one says bug `X` is open and another says it is fixed; one says Phase 12 depends on Phase 9 and another says it does not; one records the interpreter as the runtime backend and another records the JIT as the runtime backend. Each incompatible pair counts as **one contradiction**. A correctly recorded supersession — the old fact explicitly marked as replaced by the new one — is **not** a contradiction; that is the corpus correctly representing change over time.

### The integrity metric

Per arm and trial, `defects = duplicates + contradictions`, **normalised to corpus size** as `defects per 100 items` (items = files for Arm A, entities for Arm B). The signed **integrity delta is A − B** (higher = Arm A dirtier, i.e. enforcement kept Arm B cleaner), carried with its standard error across trials. Its decision band and the positive margin are in [bands.md](bands.md).

### Auditor prompt (integrity endpoint)

The machine form of this rubric, driven by the harness ([prompts.json](prompts.json) `auditor_skeleton`; the two MUST agree verbatim). It is **arm-neutral by construction** — a single skeleton, no per-arm block — because the auditor is never told which arm it audits. The harness fills `{CORPUS}` with that arm's whole corpus (Arm A the concatenated markdown bodies, Arm B the projected entity texts), **tell-stripped exactly like the judge path** so no arm identity reaches the auditor. The auditor is invoked with the pinned `auditor` model ([models.json](models.json)) and pin-honor-guarded like every other session; a trial that could not honor the pin invalidates rather than counting as zero.

```
You are auditing a knowledge base about a software project for internal defects. Below is the entire knowledge base, one item after another. Using only the definitions below, count two kinds of defect.

A duplicate is a pair of distinct items whose primary subject is the same — the same bug id, the same phase, or the same named design decision — and whose factual content substantially overlaps, so that one item is redundant given the other. Each redundant item beyond the first for a subject counts as one duplicate. Restating a fact inside a single item is not a duplicate; two items about genuinely different aspects of one subject are not duplicates.

A contradiction is a pair of items that assert incompatible facts about the same subject: one says a bug is open and another says it is fixed, or one says a phase depends on another and a second says it does not. Each incompatible pair counts as one contradiction. A correctly recorded supersession — the old fact explicitly marked as replaced by the new one — is not a contradiction.

Report only the two counts, each on its own line, as `DUPLICATES: <n>` and `CONTRADICTIONS: <n>`. Give the counts only; do not describe how you read the knowledge base or what kind of substrate holds it.

Knowledge base:

{CORPUS}
```
