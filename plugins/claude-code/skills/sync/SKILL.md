---
name: sync
description: >
  Your source changed — bring the mem up to date. Reads what changed since the
  last run plus any open findings, and updates only the affected entities,
  conservatively. The single maintenance writer for bound mems; run `--all` on
  a loop to keep every bound mem current. Not a version-control operation:
  changes flow from your source into your mem, never the reverse.
allowed-tools: Bash, Read, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_delete
argument-hint: "[--all | <binding>]"
---

# Memstead Sync

Bring a bound mem up to date with its source — the **sole maintenance writer**.
The engine renders a brief (changed slice plus open findings, conservatism
baked in); this skill runs it. Read-only on your source; write-only on the mem.

## Steps

1. Parse `$ARGUMENTS`. A binding id (`<mem>/<stem>`) → step 2. `--all` → run

   ```sh
   memstead --json projection brief --all --operation any
   ```

   No bindings configured → say so and stop. Nothing due → say "nothing due"
   and stop. Otherwise execute the returned brief faithfully per its named
   operation: verify is report-only measurement; build grows the mem (steps 2
   and 4 apply); sync maintains it (steps 2–5). No argument at all? Ask.

2. Check the anchors capability once:

   ```sh
   node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"
   ```

   **Capable** → include an `anchors` list on each create/update naming the
   source artifact(s) the entity is drawn from. **Not capable** → omit
   `anchors` and say so (with the printed reason) in your closing note.

3. Render and read the sync brief, then follow it:

   ```sh
   memstead projection brief --sync <binding>
   ```

   It carries the changed slice, the open findings, and the conservatism rules.
   If the engine refuses, surface its remedy verbatim — never work around it.

4. Apply only what the brief calls for, via the MCP mutation tools, inside the
   destination mem only. A drift finding whose meaning didn't change is an
   **annotation**, not a rewrite; a genuinely ambiguous change is **skipped,
   finding left open**, never guessed; a removal with no retrievable base
   version is **conflict-flagged** — present both sides, never auto-delete.

5. Record what you did so the baseline advances:

   ```sh
   memstead projection advance <binding> --dispositions '{"<artifact-id>":"worked", …}'
   ```

   Map every artifact the brief presented to its disposition; pass only ids the
   brief listed. The baseline advances once the slice is fully dispositioned.

## Rules

- The **sole maintenance writer** for bound mems. Changes flow source → mem,
  never mem → source; this is not a version-control operation.
- Conservative by default — the brief's rules bind. When unsure, skip and leave
  the finding for a later pass: a stale finding is cheaper than a wrong edit.
