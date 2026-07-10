---
name: verify
description: >
  Measure how faithfully a mem describes its source — a read-only fidelity
  report (coverage, accuracy, freshness) that leads with a verdict and the top
  actions. It changes nothing itself and files findings for /sync to act on.
  Not for verifying your project's changes or tests — it measures the mem.
allowed-tools: Bash, Read, mcp__memstead__memstead_health
argument-hint: "<binding>"
---

# Memstead Verify

Measure how faithfully a bound mem still describes its source and surface what
drifted — **read-only**: this skill never edits the graph. It measures the mem
and files findings; acting on them is `/sync`'s job.

## Steps

1. Resolve the binding id from `$ARGUMENTS` — the canonical form `<mem>/<stem>`
   (e.g. `docs/guide`). If none was given, ask the user which bound mem to
   measure rather than guessing.

2. Run the engine's fidelity measurement and render its report:

   ```sh
   memstead projection verify <binding>
   ```

   The command adjudicates the mem's anchors against the live source, records
   durable findings, and prints the deterministic **tier-1 fidelity report** —
   a rollup verdict, the top actions, then grain-classed coverage, anchor
   resolution, freshness, the capability block, and the backlog depth. Present
   that report; lead with the verdict and the top actions exactly as the engine
   ordered them — do not re-rank or editorialize the numbers.

3. **First run against a mem that predates its binding reads as onboarding, not
   failure.** A 0%-coverage (or near-zero) first report means the mem was built
   before it was bound to a source — that is expected. Say so plainly and name
   the backfill route (`/ingest` to grow coverage, then `/sync` to keep it
   current); never frame an adopt-run report as the mem being broken.

4. If the engine refuses (e.g. verify not enabled on the binding, or a medium
   that cannot support measurement this cycle), surface its remedy verbatim —
   it already names the one command to run (`memstead projection enable verify
   <binding>`). Do not work around a refusal.

## Rules

- Report-only. This skill mutates nothing — no entity create/update/delete/
  relate, on any path. Findings live in the engine-owned store, never in a
  skill-side file. A repair is a separate job: `/sync`.
- It measures the **mem**, not your project's code or tests — if the user wants
  their own changes checked, that is not this skill.
