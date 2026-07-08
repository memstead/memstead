# Drift-closure self-proof — method & source

This note is the public method/source for the drift-closure figure Memstead
publishes about itself: **79% of 153 drift findings closed, at a ~12-minute
median open-to-close time.** It is one measured run over the project's own live
graph — a self-proof, not a benchmark, and not a claim that drift is "solved."

## What is being measured

Memstead develops in the open and keeps its own specifications and decisions as
live mems. A set of **ingest process-mems** runs an issue-tracker lifecycle over
*destination-quality drift*: each time the source code moves ahead of what a
knowledge mem records, a drift finding is opened; when the mem is brought back
into agreement, the finding is closed (deleted with a `Resolved: …` rationale in
the commit body). The figure measures how much of that drift the loop actually
closed, and how quickly.

This is the falsification clause of the project's layered-freshness bet made
concrete: if the drift-detection layer were not keeping the mems synchronised
during active development, the closure rate would show it.

## Method

Read-only commit-mining over the live mem-repo (2026-06-11):

- All **131 delete commits** across the mem-repo's **ten entity branches** were
  mined.
- The ingest process-mems account for **153 drift findings** opened since the
  registers began.
- **79%** of those findings were closed; the **median open-to-close time was
  ~12 minutes**.
- **101 of the 131** deletes carry a `Resolved: …` rationale note in the commit
  body.

A note on churn: the raw delete rate initially read as ~32%, which looked like
instability. Mining resolved it into a **two-tier metabolism** — the knowledge
mems themselves are near-append-only (10 lifetime deletes, of which 9 were a
single operator-requested early wipe of the engine mem and exactly 1 a
substantive redundancy cleanup), while the fast-cycling churn lives entirely in
the drift-findings layer. Stable knowledge, fast-cycling findings.

## Honest bounds

- **One run, one graph.** These are numbers from Memstead's own repository at one
  point in time, not a controlled comparison against other tools.
- **Detection is not repair.** The loop *finds* drift reliably; closing a finding
  still takes a human pass on the source side. Automated detection is the part
  that runs; end-to-end auto-repair does not.
- **Coverage is partial.** The freshness mechanisms bind drift only where they
  run — a minority of the project's own surfaces so far. The self-proof is in
  progress, not achieved.

## Reproducing it

The measurement is a read-only count against the mem-repo's git branches and can
be re-run at any time; no third party has independently reproduced it yet (which
would lift it from *repeatable* to *proven*). The commit-mining method and the
per-branch split are recorded in the project's own status-site campaign protocol
(`dev/status-site-campaign/protocol.md`, Iteration 18 / Finding F19).

## Where this figure lives

The number rendered on [memstead.com](https://memstead.com) is generated from the
project's graph, not hand-typed — see `websites/memstead.com/scripts/build-scoreboard.mjs`.
Its source of record is the launch-claims register assertion
*"The measured drift loop closed 79 percent of 153 findings at a 12-minute
median."*
