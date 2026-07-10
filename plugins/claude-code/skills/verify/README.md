# /verify — measure a mem's fidelity to its source

`/verify <binding>` runs the engine's read-only fidelity measurement over a
bound mem and presents the deterministic tier-1 report: a rollup verdict and top
actions, then grain-classed coverage, anchor resolution, freshness, the
medium-capability block, and the tier-3 backlog depth.

It is **report-only** — it never edits the graph. It adjudicates the mem's
anchors against the live source and records durable findings in the
engine-owned findings store; acting on those findings is `/sync`'s job.

- Takes the canonical binding id `<mem>/<stem>` (e.g. `docs/guide`).
- A near-zero first report against a mem that predates its binding is
  **onboarding, not failure** — backfill with `/ingest`, then keep current with
  `/sync`.
- It measures the **mem**, not your project's code or tests.

Under the hood it shells `memstead projection verify <binding>`; a refusal
(verify not enabled, or a medium that cannot support measurement this cycle)
carries the one-command remedy, surfaced verbatim.
