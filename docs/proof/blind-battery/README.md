# Blind battery: a mem-only reader against a source-only reader

Eleven fixed questions about Memstead. For each question two fresh agents
answer: one may read only the installed engine mem through the memstead
CLI, the other only the Rust source under `crates/`. The answers are
extracted, their citations stripped, and placed in a random A/B order. A
third fresh agent grades each pair against the source and by running the
release binary, and returns a JSON verdict. The recorded order maps each
verdict back to mem or source.

The instrument is `ci/blind_battery.py`; the questions are embedded there
and stay fixed so that runs compare. It measures the mem, not the reader:
the same model, the same word cap and the same grader stand on both sides,
and the only thing that differs is what each reader may read. A win for the
source is a list of claims the mem got wrong or lacks; a win for the mem is
a place where the curated graph beats reading the code.

## Runs

| Run | Engine mem | Wins (mem / source / tie) | Record |
|---|---|---|---|
| 2026-09-04 | 0.2.2 as published, unsynced | 4 / 7 / 0 | the coordinating session's scratch record; not preserved beyond the verdict lists in the graph |
| 2026-09-06 | claim-walked sync of the same day (engine 28421da, mem branch `memstead/engine` at baea51ed), exported self-contained | 2 / 9 / 0 | [2026-09-06/](2026-09-06/), [result.md](2026-09-06/result.md) |

The first run's losses for the mem traced to sentences about source files
its entities never anchored, standing falsified for weeks; the engine moves
that ended that class (the sync brief steers by mention, verify reports an
unanchored mention) and the claim-walked sync that followed are what the
second run measures.

The second run went further against the mem than the first: the mem won
Q2 (what an installed mem can and cannot do) and the small-model repeat of
Q1 (concurrent writes), the source won the other nine, and the margin sum
was 15 to 3. The graders' error lists on the mem side are the next sync's
work list; several of them are the reader's own inference over a correct
entity, and the record keeps them apart only by reading the cited pair.

Three pairs of the 2026-09-06 run were graded twice. The first pairing
stripped every `<mem>--<slug>` id, which removed the scratch mem's ids from
both command sequences of N1, and rewrote the phrase "entity id" as
"references", which put one false field name into both answers of N1, N5
and Q0h. The stripping was corrected (only the mem under test's ids are
citations; the phrase rule is gone), the three pairs were re-paired and
graded again by fresh graders, and the result counts the second verdicts.
The first verdicts stay in the record as `verdicts/<key>.first-strip.json`
(and the first N1 pairing as `pairs/N1.first-strip.md`): all three named
the same winner as the regrade.

## Rerun kit

```sh
# a self-contained archive of the mem under test
memstead --workspace graph export --format mem --mem engine --self-contained -o engine.mem
python3 ci/blind_battery.py prepare --dir RUN --memstead target/release/memstead \
    --mem engine.mem --source . --seed 20260906
python3 ci/blind_battery.py run --dir RUN --phase readers
python3 ci/blind_battery.py pair --dir RUN
python3 ci/blind_battery.py run --dir RUN --phase graders
python3 ci/blind_battery.py tally --dir RUN
```

`run` drives the readers and the graders through `claude -p` with the tool
allow-lists the protocol requires (the mem reader gets the memstead binary
and nothing else, the source reader gets read and search over `crates/`,
the grader gets the binary and a scratch workspace). A session that spawns
its own agents skips `run`: it hands each agent the prompt under
`RUN/prompts/` and drops the reply into `RUN/answers/<key>-<side>.md` or
`RUN/verdicts/<key>.json`; `pair` and `tally` are the same either way.

A run directory holds `index.json` (the questions, the seeded order, the
models, the binary version, per-call token and turn counts where the driver
ran), `prompts/`, `answers/`, `pairs/`, `verdicts/`, `result.json` and
`result.md`. `record --dir RUN --out docs/proof/blind-battery/<date>` writes
the committed form: the same files with the machine's paths replaced by
`<run>`, `<memstead>`, `<source>` and `<mem>`, without the reader workspace
and the graders' scratch directories.

## Reading a result

`result.md` states wins per side, the margin sums, the score totals per
axis (correctness, completeness, usefulness, honesty), the wins split by
model size, and the graders' error lists per side. The error list on the
mem side is the next sync's work list: each entry names a claim a grader
refuted against the source or the binary.
