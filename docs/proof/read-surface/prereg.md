# Read-surface headroom, pre-registration

**Status: pre-registered, apparatus built and smoke-tested, full run NOT started
(2026-08-25).** Three smoke passes ran and are recorded as amendments A1 to A3
below; no scored arm of the full battery has run, and no result exists. The
operator stopped short of the full run after the smoke passes showed how much of
a model-judged battery's output depends on apparatus that fails silently. Whether
it runs at all is open. The package stands as written so that a later session can
execute it without re-deciding anything, and so the amendments stay auditable.

**Committed before any scored arm runs.** Nothing in this file
changes after the first arm except by a dated amendment note appended at the
bottom, stating the date, what changed, and why. The published report must
surface every amendment verbatim.

This is a **new, separate experiment**, not an amendment to the
[divergence campaign](../divergence/README.md). That campaign's pre-registration
package is frozen and is not touched by anything here.

## The question

The divergence campaign measured that the engine-gated arm answered its query
battery worse than the tolerant markdown arm, and that the gap widened across
ten rounds. That campaign changed **two** things between its arms at once: the
storage form and the access surface. Its own pre-registration says so plainly:

> The comparison is deliberately substrate-**with-its-native-access-surface**,
> not bytes-with-identical-tools. […] The consequence is stated plainly:
> **tool-surface differences are part of the measured variable, not a confound to
> be denied.**

That was a defensible choice for a moat question. It leaves one thing unknown:

> **Was the accuracy loss caused by the typed bytes, or by the engine's read
> tools? And how much read-side improvement is available on the same bytes
> without changing the substrate at all?**

This experiment holds the bytes fixed and varies only the access surface. It is
a **diagnostic, not a verdict test**: its output is a distance, and a distance
is a direction.

## What is held constant

Everything except the access surface:

- **The corpora.** The frozen round-10 outputs of the divergence campaign: 81
  entity markdown files in `../divergence/arm-b-mem/`, 41 in `../divergence/arm-a/`.
  No writer runs. Nothing is authored, edited or regenerated. Every arm reads a
  copy; the committed originals are never touched by a run (see control 3).
- **The questions.** The same pre-registered twelve-query battery with its
  reference answers, `../divergence/prereg/queries.json`, unchanged.
- **The reader prompt skeleton**, byte-identical across all five arms, taken
  verbatim from `../divergence/prereg/prompts.json`:

  > Answer the following question about the software project, using only the
  > knowledge base. Answer the question directly and concisely: state the answer
  > itself, not how you found it, which tools you used, or where in the knowledge
  > base it is stored. If the knowledge base does not contain enough information
  > to answer, say so plainly.
  >
  > `{SUBSTRATE_BLOCK}`
  >
  > Question: `{QUERY}`

- **The parity contract.** Only the substrate block differs between arms, and it
  contains **only** access mechanics: never a quality exhortation, never a
  mention that a measurement is happening, never a hint about what the queries
  will ask.
- **Trials:** 3 per query per arm. **Reader budget:** 8,000 tokens per answer.
- **Blinding:** every answer is tell-stripped with the union of both tell lists
  (`../divergence/prereg/tell-lists.json`) before it reaches the judge, exactly as
  in the campaign.
- **The judge:** the campaign's blind judge and rubric
  (`../divergence/prereg/rubrics.md`), unchanged, scoring against the same
  reference answers.

## The five arms

| # | Name | Corpus | Access surface |
|---|---|---|---|
| 1 | engine | copy of `arm-b-mem/`, mounted as a mem | the three engine read tools only |
| 2 | files | the 81 entity files, stripped copy | `Read, Grep, Glob, LS` |
| 3 | dump | the 81 entity files, concatenated | none; the corpus is in the prompt |
| 4 | shell | the 81 entity files, stripped copy | `Read, Grep, Glob, LS, Bash` |
| 5 | control | copy of `arm-a/`, the 41 tolerant files | `Read, Grep, Glob, LS` |

Arms 1 and 5 reproduce the campaign's two reader configurations. They are re-run
rather than quoted, so no result here depends on the July run's model still being
served, and all five numbers are produced under identical conditions on one day.

**Arms 2 and 5 receive the byte-identical substrate block.** The only difference
between them is which directory they are pointed at. That pair is the cleanest
isolation of the substrate variable this experiment offers, and it is the pair
that answers the question directly.

### Substrate blocks

Arms 2 and 5, verbatim from the campaign package (`reader_substrate.arm_a`):

> The knowledge base is a directory of markdown files. Read it with your
> filesystem tools.

Arm 1, verbatim from the campaign package (`reader_substrate.arm_b`):

> The knowledge base is a Memstead mem. Read it with `memstead_overview`,
> `memstead_search`, and `memstead_entity`.

Arm 4, new. The minimum edit to the arms-2/5 block that names the added
mechanic, and nothing else:

> The knowledge base is a directory of markdown files. Read it with your
> filesystem and shell tools.

Arm 3, new. No tool mechanic exists to describe, so the block carries the corpus:

> The knowledge base is reproduced in full below.
>
> `{CORPUS}`

## Controls on the two new arms

Two ways these arms could quietly stop measuring what they claim to measure, both
closed before the run:

1. **Engine leakage into arms 2, 3 and 4.** A shell-armed reader could invoke the
   `memstead` CLI if it is on `PATH`, which would make arm 4 an engine arm
   wearing a shell costume. Arms 2, 3 and 4 run with a `PATH` from which both
   `memstead` and `memstead-mcp` are absent, and the run records the `PATH` it
   used.
2. **Engine metadata as a free extra source.** `arm-b-mem/` carries a `.memstead/`
   directory holding config, mount state and a change log. A markdown directory
   has no such thing, so leaving it visible would hand arms 2, 3 and 4 a source
   arm 5 does not have. Those three arms run against a copy containing **only the
   81 entity `.md` files**.
3. **Mounting mutates the corpus.** Found while checking feasibility on
   2026-08-25, before any arm ran: pointing the current binary at
   `arm-b-mem/` with a plain read command (`memstead status`) rewrites
   `.memstead/meta-schemas/` to the running binary's version. A read operation
   is therefore not side-effect-free on the directory it reads. **Every arm,
   arm 1 included, runs against a fresh copy**, and the committed corpora are
   never the working directory of a run. The copies are made once, before the
   first arm, and all five arms are served from that same generation of copies.

## The two measures

Let `s1` to `s5` be each arm's mean score over 12 queries by 3 trials, aggregated
the way the campaign aggregates: the mean of the per-query means, with the
standard error taken as the population standard deviation of the per-query values
divided by the square root of twelve.

- **Headroom**, `H = max(s2, s3, s4) − s1`. How much better the same bytes can be
  read without touching the substrate.
- **Substrate gap**, `G = max(s1, s2, s3, s4) − s5`. Whether typed bytes can
  outread flat bytes at all, given the best surface tried here.

## What each outcome means, fixed in advance

**On headroom.**

- `H ≥ +0.10` and `H` minus its standard error is above zero: the engine's read
  tools left measurable ground on the table on this corpus. The winning arm names
  what the read surface should do, and the distance sizes the work.
- `H` inside ±0.05: the read tools are not the bottleneck. The campaign's
  accuracy loss is not a tooling artifact, and no read-surface work is justified
  by this measurement.
- `H ≤ −0.10`: the engine's read tools genuinely beat generic file access on
  typed bytes, which is the one outcome that would argue for the current surface
  as built.

**On the substrate gap.**

- `G > 0` and `G` minus its standard error is above zero: typed bytes outread
  flat bytes once the surface is right. The campaign's accuracy result is then a
  read-surface finding, and should be cited as one.
- `G` inside ±0.05: typed structure is neutral for reading on this corpus. The
  honest statement becomes that the engine's value is on the write side, and the
  read side is neither an asset nor a liability.
- `G < 0` and `G` plus its standard error is below zero: with the best of four
  surfaces, typed bytes still do not outread flat bytes. The substrate reading of
  the campaign result is confirmed, and the read side is closed on this corpus for
  reasons that are not about tooling.

The two measures are independent and are reported independently. Any combination
is reportable; none is a failure of the experiment.

## Publication rule

All five arms are published with their numbers, whatever they show. No arm is
dropped, re-run, or reinterpreted after its result is seen. If an arm cannot run
for a mechanical reason, that is reported as a limit of the experiment with the
reason, never omitted. The per-arm token ledger is published alongside the
scores.

## Model

The campaign pinned `claude-opus-4-8` for reader and judge
(`../divergence/prereg/models.json`). This experiment tries that pin first. If it
is no longer served, **every arm** runs on one current model instead and the
report states which. Because all five arms are re-run here, internal
comparability holds under either branch; only comparison to the July numbers
depends on the pin.

## Cost and stop rule

180 reader sessions and 180 judge calls (5 arms by 12 queries by 3 trials). The
campaign's whole reader plus judge spend was on the order of 65M raw tokens for
roughly 1.6 times this volume, almost all of it cache reads, so this run is small.
A smoke pass of one query across all five arms runs first and must show: the model
resolves, arm 3's corpus fits the context, the mem mounts for arm 1, and the
stripped `PATH` holds for arms 2 to 4. If the smoke pass fails, the failure is
recorded here as an amendment before anything is changed.

## What this experiment cannot show

- **It is one corpus**, of 81 and 41 entities, on one topic, produced by one
  campaign. It says nothing about behaviour at the engine's design scale of one
  to five thousand entities, and nothing about reading across several mems, which
  no measurement in this project has ever covered.
- **It does not test writing.** The corpora are frozen. Nothing here bears on the
  campaign's integrity result, in either direction.
- **The four surfaces tried are not the space of possible surfaces.** A negative
  headroom result bounds what these four can do, not what any surface could.
- **Arm 3 depends on the corpus fitting the context window**, which it does at
  this size and would not at the engine's design scale. Its result does not
  generalise upward.

## Amendments

Three smoke-pass failures and one pre-declared confound, all dated 2026-08-25,
all recorded before the first scored arm of the full run.

**A1 — the harness could not resolve `claude`.** The first smoke pass never
reached a model: the child process failed to find the `claude` executable on
`PATH` even though the invoking shell resolves it. A `--claude-binary` flag was
added, defaulting to bare `claude`, so the run can be handed an absolute path.
No experimental parameter changed. Noted for anyone re-running the divergence
campaign: its harness hardcodes `claude` and will hit the same wall.

**A2 — the harness discarded the evidence needed to read its own scores.** The
second smoke pass returned 0.000 on four arms and there was no way to tell an
honest "the knowledge base does not say" from an answer the blinder had
shredded, because only the score was kept. Every session now persists its raw
answer, its tell-stripped form, its tool calls and its score. This is the
campaign's own practice, which the first draft of this harness failed to copy.
A score published without the text behind it is not interpretable, and this
package will not publish one.

**A3 — the engine arm was not reaching the engine.** In the second smoke pass
the engine arm made no `mcp__memstead__*` call at all and answered "I'll use the
Grep tool instead": the generated MCP config set the server's working directory
with a `cwd` key, which the config format does not honour, so `memstead-mcp`
never found the mem. The config now wraps the binary in a shell that changes
directory first, the form the campaign's own committed config uses. A guard was
added with it: an engine-arm session that records no memstead tool call aborts
the run instead of scoring. An arm that silently degrades into a different arm is
the worst failure this design can suffer, so it now fails loudly. Verified in the
third smoke pass, which recorded `memstead_overview`, `memstead_search`,
`memstead_schema` and `memstead_entity` calls.

**A4 — a coverage confound in the inherited battery, declared before the run.**
The smoke pass surfaced it and it is stated here rather than discovered in the
results. **Five of the twelve inherited queries ask about bug records**
(`S1-open-bugs`, `S2-open-high-sev`, `A1-codegen-bug-count`, `A2-ledger-totals`,
`A3-open-codegen-bugs`), and the typed corpus largely does not carry that
material: 26 distinct bug identifiers against the tolerant corpus's 244, spread
across 10 files against 26, and **zero entities of the schema's `incident`
type**. In the smoke pass all four typed arms, including the dump arm holding the
entire corpus in context, independently answered that the knowledge base does not
contain it; the control answered from a `bug-tracker.md` file and scored 0.85.

A query whose answer is absent from a corpus cannot discriminate between surfaces
over that corpus: every surface scores zero, and the measure carries no
information about the surface. The response is additive, never substitutive:

- **The primary measures stay exactly as registered, over all twelve queries.**
  Headroom and substrate gap are computed and published over the full battery.
  Nothing is dropped.
- **A secondary analysis over the seven non-bug queries is added**, computed the
  same way and labelled as secondary wherever it appears. It exists because the
  seven are where a read-surface difference can show at all.
- **Both are published together, with the per-query table**, so a reader can see
  which queries carried which measure.

This is declared now, before the full run, so it cannot become a post-hoc subset
chosen for its result. If the two analyses disagree, both are reported and the
disagreement is the finding.
