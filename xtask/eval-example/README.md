# Substrate-quality eval — runnable example

A self-contained corpus + task + fact set for the substrate-quality harness
(`engine/xtask/src/eval/`). The corpus is **fictional** (the "Zephyrine Relay
Mesh") so the contamination guard's no-substrate (A) arm cannot answer the tasks
from prior knowledge — every task survives the screen, and the B-vs-C delta speaks
only to substrate quality.

- `corpus.md` — the one source corpus both arms capture.
- `tasks.json` — `[{id, prompt, reference}]`, answerable only from the corpus.
- `facts.json` — `[{id, statement}]`, ground-truth source facts for the coverage signal.

## Run a real B-vs-C comparison end-to-end (single command)

Needs a live `claude` CLI on `$PATH` and the two full binaries built
(`cargo build --features mem-repo -p memstead-mcp` and `cargo build -p memstead`).
The harness **self-provisions** the empty destination mem for the schema-forced
arm — no manual setup. Run from the repo root:

```sh
ROOT=$(pwd)
cargo run -p xtask -- eval \
  --subject zrm \
  --tasks   "$ROOT/engine/xtask/eval-example/tasks.json" \
  --facts   "$ROOT/engine/xtask/eval-example/facts.json" \
  --capture-corpus "$ROOT/engine/xtask/eval-example/corpus.md" \
  --capture-workspace /tmp/eval-capture-mem \
  --cli-binary  "$ROOT/engine/target/debug/memstead" \
  --mcp-binary  "$ROOT/engine/target/debug/memstead-mcp" \
  --model claude-sonnet-4-6 \
  --trials 1 \
  --contamination-threshold 0 \
  --output /tmp/zrm-series.json
```

This captures both substrates from the corpus (free-reason-then-write; the
schema-forced arm writes typed entities into the provisioned mem via MCP, the
free-form arm writes notes), screens the tasks against the no-substrate arm, answers
each task from each substrate placed wholly in context (retrieval held out), grades
blind, measures coverage, and writes a chart-ready series to `--output`. The signed
`delta` (C − B) is reported as-is — a flat or negative result is not floored.

Two notes the example bakes in deliberately:

- **`--mcp-binary` must resolve to a real file** — the harness canonicalises it to an
  absolute path before embedding it in the generated mcp-config (which runs
  `cd <mem> && exec <mcp-binary>`), so a relative path from the repo root works.
- **`--contamination-threshold 0` disables the no-substrate screen** for this demo.
  This toy corpus is small enough that the bare model sometimes fabricates a
  judge-passing answer, so at the default threshold (`0.5`) the guard may flag a task
  as guessable and exclude it — possibly aborting with "the corpus is fully
  guessable". That is the contamination guard *working as designed*; for a real
  measurement use a corpus the bare model cannot answer and leave the screen on.
