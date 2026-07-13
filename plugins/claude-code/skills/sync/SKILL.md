---
name: sync
description: >
  Your source changed — bring the mem up to date. Reads what changed since the
  last run plus any open findings, and updates only the affected entities,
  conservatively. The single maintenance writer for bound mems; run `--all` on
  a loop to keep every bound mem current, or `--verify <binding>` for a
  read-only fidelity report (coverage, accuracy, freshness) that changes
  nothing. Not a version-control operation: changes flow from your source into
  your mem, never the reverse.
allowed-tools: Bash, Read, mcp__memstead__memstead_schema, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_delete
argument-hint: "[--all | <binding> | --verify <binding>]"
---

# Memstead Sync

Bring a bound mem up to date with its source — the **sole maintenance writer**
— or measure it read-only with `--verify`. Read-only on your source;
write-only on the mem. If the engine refuses anything, surface its remedy
verbatim — never work around it.

## Steps

1. Parse `$ARGUMENTS`. A binding id (`<mem>/<stem>`) → step 2. `--verify
   <binding>` → step 6. No argument? Ask. `--all` → run

   ```sh
   memstead --json projection brief --all --operation any
   ```

   No bindings configured → say so and stop. Nothing due → say so and stop.
   Otherwise execute each returned brief faithfully per its named operation:
   verify is report-only measurement; build grows the mem — a build brief IS
   the sanctioned backfill channel, so cover the batch it asks for rather
   than declining it as out-of-scope (steps 2 and 4 apply); sync maintains
   it (steps 2–5).

2. Check the anchors capability once:

   ```sh
   node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"
   ```

   **Capable** → include an `anchors` list on each create/update naming the
   source artifact(s) the entity is drawn from. **Not capable** → omit
   `anchors` and say so (with the printed reason) in your closing note.

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
   a mem older than its binding is onboarding, not failure — name the backfill
   route (`/ingest` to grow coverage, then `/sync` to keep it current).

## Rules

- The **sole maintenance writer** for bound mems. Changes flow source → mem,
  never mem → source; this is not a version-control operation.
- Conservative by default — the brief's rules bind. When unsure, skip and leave
  the finding for a later pass: a stale finding is cheaper than a wrong edit.
