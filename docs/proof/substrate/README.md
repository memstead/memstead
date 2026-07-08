# Substrate eval — does typing beat equally-curated notes? (2026-07-08)

A controlled measurement over Memstead's own knowledge, asking one question:

> When the same source material is captured two ways — as **schema-forced typed
> entities** (C) and as **equally-curated free-form markdown notes** (B) — does
> the typed form let an agent answer questions about it any better?

It is **one observed run, not a benchmark.** Everything needed to audit and rerun
it is in this folder, and the inputs were committed *before* a single arm ran.

## What it supports, and what it does not

- **It supports:** the honest, narrow reading of Memstead's value. On this run,
  typing bought **no measurable answer-time advantage** over good flat notes when
  the whole corpus fits in context — which is exactly what you'd expect if the
  payoff of typed structure is at **write-time enforcement, scale, and
  governance**, not at answer-time recall. The number is published as-is.
- **It does not support:** any claim that the typed graph makes an agent *answer
  better* in this setting, or that it *compresses* better than curated notes. If
  anything the tiny delta leans the other way, well within noise.
- **It does not touch:** the separate ~1/13 [reconstruction measurement](../reconstruction/).
  That compares a curated mem against *raw authoring conversations* — it measures
  **curation**, not **typing**. This eval is the control that the reconstruction
  run lacked: it isolates the typing variable, and finding it flat means the 1/13
  compression should be attributed to curation (which flat notes share), **not**
  to the typed graph specifically.

## The result

Signed substrate delta **C − B = −0.010 ± 0.006** (stderr), over the 5 tasks that
survived the contamination screen, 3 trials each:

| Metric | Schema-forced (C) | Free-form (B) | Δ (C−B) |
|---|---:|---:|---:|
| Mean task score | 0.973 | 0.983 | **−0.010** |
| Fact coverage (of 22) | 22/22 (100%) | 22/22 (100%) | 0 |

Per task (mean over 3 trials):

| Task | C | B | Δ |
|---|---:|---:|---:|
| concurrent-writes | 1.000 | 1.000 | 0.000 |
| sibling-coherence | 0.967 | 0.983 | −0.017 |
| bad-write-response | 0.967 | 0.967 | 0.000 |
| portability-copy | 1.000 | 1.000 | 0.000 |
| provenance-rate | 0.933 | 0.967 | −0.033 |

The delta is **statistically indistinguishable from zero** (|Δ| < 2× its stderr,
and the per-task signs are mixed). Both arms answer the corpus nearly perfectly;
the storage form did not separate them.

## Method

The substrate mode of the eval harness (`cargo run -p xtask -- eval`), designed so
the only difference between the arms is the storage form:

- **Same sources.** Both substrates are captured from the identical corpus
  (`corpus.md`), by the same model, with a shared free-reasoning instruction and
  a storage-specific write step. Free-form writes lightly-structured notes;
  schema-forced maps the *same* free extraction into typed entities via the
  engine, then reads them back. The harness refuses a capture pair that diverges
  in model, reasoning prompt, or sources — so the schema arm cannot be handed an
  unfair head start.
- **Both substrates in context, retrieval held out.** Each arm answers with its
  whole substrate in the prompt under the same 8,000-token budget, so the test is
  about the *form* of the knowledge, not a retrieval race.
- **Blind judge.** An unlabeled judge scores each answer against a reference; it
  is not told which arm produced the answer.
- **Contamination screen.** Before scoring, a no-substrate (bare model) arm
  answers every task; any task the bare model already answers at ≥ 0.5 is
  excluded as guessable. One of six was excluded here (`why-engine-only`, 0.53),
  leaving 5 tasks in the delta.
- **Variance.** 3 trials per arm per task; the table reports means and the
  aggregate carries a standard error.
- **Model:** `claude-opus-4-8`. **Trials:** 3. **Budget:** 8,000 tokens.
  **Contamination threshold:** 0.5.

## One task, both arms (from the run)

The harness persists each kept task's first-trial answer from both arms
(`transcripts` in `result.json`). Here is `provenance-rate` verbatim — a fair
picture of the whole run: both answers are correct and well-formed, and the
free-form arm is, if anything, marginally fuller.

**Q.** How much of the project's mutation history carries a human-readable reason
for each change, and what makes that happen?

**Schema-forced (C) — scored 0.933:**
> Roughly 97% of the mem-repo's mutating commits carry a one-sentence,
> human-readable prose rationale explaining why each change was made.
>
> What makes that happen is the workspace's `require_notes` policy. It nudges
> every mutating commit to include a one-sentence prose rationale in the commit
> body — placed between the mechanical subject line and the auto-emitted
> provenance trailers — capturing the "why" of a change rather than only the
> "what." Because it nudges rather than hard-enforces, adherence is high but not
> total, leaving a small remainder of commits (mostly pure-housekeeping edits)
> without a note.

**Free-form (B) — scored 0.967:**
> About 97% of the project's mutating commits carry a one-sentence,
> human-readable rationale — the *why* behind the change, not just the *what*.
>
> What makes that happen is the workspace's `require_notes` policy, which nudges
> every mutating commit to include that one-sentence prose reason. (Separately,
> each mutation is a git commit and automatically gets a trailer block with
> `Tool:`, `Actor:`, and `Client:` lines, but those record the *what/who*; the
> prose note is what supplies the *why*.) The payoff is that the project's
> decision history can be read years later with `git log` alone — no engine or
> database required.

That the two are hard to tell apart *is* the finding.

## Honest bounds (counterevidence)

- **One run, one small corpus, one model.** Not a benchmark; not a distribution.
- **The corpus fits the budget**, so neither arm had to drop anything — both
  covered 22/22 source facts. This run therefore measures **answer-time
  structuring only**, and says nothing about **compression / information-loss**,
  which only bites when the corpus is larger than the budget. A follow-up with a
  corpus that overflows the budget would test that axis.
- **Answer-time only.** It is silent on the axes where typed structure is claimed
  to earn its keep: refusing bad writes at ingest, staying navigable at thousands
  of entities, and governing who may write what. A flat answer-time delta is
  consistent with — not evidence against — value on those axes.
- **The strong model is a confound in the honest direction:** a capable model
  reads good flat notes about as well as typed entities when both fit in context.
  That is the finding, not a defect.

## Rerun kit

From the repo root, with the `memstead` and `memstead-mcp` binaries built
(`cd .. && cargo build --release -p memstead-cli -p memstead-mcp`), and the
`claude` CLI on PATH:

```
cargo run -p xtask -- eval \
  --subject mem-repo-trust \
  --capture-corpus docs/proof/substrate/corpus.md \
  --cli-binary  target/release/memstead \
  --mcp-binary  target/release/memstead-mcp \
  --tasks docs/proof/substrate/tasks.json \
  --facts docs/proof/substrate/facts.json \
  --output docs/proof/substrate/result.json \
  --trials 3 --token-budget 8000
```

## Files

| File | What it is |
|---|---|
| `corpus.md` | The source corpus both substrates are captured from (pre-registered). |
| `tasks.json` | The six tasks — question + reference answer (pre-registered, committed before the run). |
| `facts.json` | 22 atomic ground-truth facts for the coverage measurement (pre-registered). |
| `result.json` | The harness output: per-task and aggregate scores, variance, the signed delta, contamination exclusions, per-substrate fact coverage, and the per-arm first-trial answer transcripts. |
| `README.md` | This note. |
