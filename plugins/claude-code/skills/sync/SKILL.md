---
name: sync
description: >
  Your source changed — bring the mem up to date. Reads what changed since the
  last run plus any open findings, and updates only the affected entities,
  conservatively. The single maintenance writer for bound mems; run `--all` on
  a loop to keep every bound mem current, `--verify <binding>` for a
  read-only fidelity report (coverage, accuracy, freshness) that changes
  nothing, or `--inventory <binding>` for the on-demand full stock-take —
  measure the whole binding, repair to quiescence, report. Not a
  version-control operation: changes flow from your source into your mem,
  never the reverse.
allowed-tools: Bash, Read, mcp__memstead__memstead_schema, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_delete
argument-hint: "[--all | <binding> | --verify <binding> | --inventory <binding>]"
---

# Memstead Sync

Bring a bound mem up to date with its source — the **sole maintenance
writer** — or measure it read-only with `--verify`. Read-only on your source,
write-only on the mem; engine refusals are surfaced verbatim, never worked around.

## Steps

1. Parse `$ARGUMENTS`. A binding id (`<mem>/<stem>`) → step 2. `--verify
   <binding>` → step 6. `--inventory <binding>` → step 7. No argument? Ask.
   `--all` → run

   ```sh
   memstead --json projection brief --all --operation any
   ```

   No bindings configured → say so and stop. Nothing due → say so and stop —
   and under a recurring loop (a scheduler re-prompting this conversation), a
   second consecutive nothing-due rotation means the catch-up job is DONE:
   cancel the schedule driving the loop and report quiescence in one line (a
   standing watch is a deliberate restart at a slower cadence).
   Otherwise execute each returned brief faithfully per its named operation:
   verify is report-only measurement; build grows the mem — a build brief IS
   the sanctioned backfill channel, so cover the batch it asks for rather
   than declining it as out-of-scope (steps 2 and 4 apply); sync maintains
   it (steps 2–5).

2. Check the anchors capability once:

   ```sh
   node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"
   ```

   **Capable** → include `anchors` on each create/update naming the source
   artifact(s) drawn from. **Not capable** → omit and say so with the reason.

3. Render and read the sync brief, then follow it — it carries the changed
   slice, the open findings, and the conservatism rules:

   ```sh
   memstead projection brief --sync <binding>
   ```

4. Apply only what the brief calls for, via the MCP mutation tools, inside the
   destination mem only. A drift finding whose meaning didn't change is an
   **annotation**, not a rewrite; a genuinely ambiguous change is **skipped,
   finding left open**, never guessed; a removal with no retrievable base
   version is **conflict-flagged** — present both sides, never auto-delete.

5. Record what you did so the baseline advances:

   ```sh
   memstead projection advance <binding> --dispositions '{"<artifact-id>":"worked", …}'
   ```

   Anchored writes count as worked on their own; supply dispositions only for
   the rest (skipped or out-of-intent artifacts), using only ids the brief
   listed. The baseline advances once the slice is fully dispositioned.

6. `--verify <binding>`: run `memstead projection verify <binding>` —
   report-only, nothing changes; it measures the mem, not your project's
   changes or tests. Present the engine's deterministic report as ordered —
   verdict and top actions first, never re-ranked. A near-zero first report on
   a pre-binding mem is onboarding — name the route (`/ingest`, then `/sync`).

7. `--inventory <binding>`: the full stock-take — measure completely, then
   repair to quiescence. Start with the complete measurement:

   ```sh
   memstead projection verify <binding> --full
   ```

   Then repair in passes: steps 2–5 off the rendered sync brief, then
   re-run the verify above, and repeat. Done when the brief reports nothing
   to sync AND the re-verify is clean or every remaining finding carries a
   disposition. **Hard rule — progress must be monotone.** Count the open
   work before each pass (open findings plus artifacts still awaiting
   disposition); a pass that does not strictly shrink that count ends the
   run with an honest "did not converge" report naming the stuck items —
   never another pass over them, never a silent loop. Keep no state of your
   own between passes — the engine's dispositions are the resume point. Close
   with the final fidelity report presented as in step 6 — verdict first.

## Rules

- The **sole maintenance writer** for bound mems. Changes flow source → mem,
  never mem → source; this is not a version-control operation.
- Conservative by default — the brief's rules bind. When unsure, skip and leave
  the finding for a later pass: a stale finding is cheaper than a wrong edit.
