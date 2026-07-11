---
name: sync
description: >
  Your source changed — bring the mem up to date. Reads what changed since the
  last run plus any open findings, and updates only the affected entities,
  conservatively. The single maintenance writer for bound mems. Not a
  version-control operation: changes flow from your source into your mem, never
  the reverse.
allowed-tools: Bash, Read, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_delete
argument-hint: "<binding>"
---

# Memstead Sync

Bring a bound mem up to date with its source — the **sole maintenance writer**.
The engine renders a sync brief (what changed since the last sync plus any open
findings, with the conservatism baked in); this skill runs it and applies what
it calls for. Read-only on your source; write-only on the mem.

## Steps

1. Resolve the binding id from `$ARGUMENTS` (`<mem>/<stem>`, e.g. `docs/guide`).
   None given? Ask which bound mem to sync rather than guessing.

2. Check the anchors capability once:

   ```sh
   node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"
   ```

   It prints `{ "capable": …, "reason": … }`. **Capable** → include an `anchors`
   list on each create/update naming the source artifact(s) the entity is drawn
   from. **Not capable** → omit `anchors` and say so in your closing note (use
   the printed reason). Never send anchors just to probe whether they work.

3. Render and read the sync brief, then follow it:

   ```sh
   memstead projection brief --sync <binding>
   ```

   It carries the changed slice, the open findings, and the conservatism rules.
   If the engine refuses (sync not enabled, unsupported medium), surface its
   remedy verbatim (`memstead projection enable sync <binding>`) — don't work
   around a refusal.

4. Apply only what the brief calls for, via the MCP mutation tools, inside the
   destination mem only. Judgment the brief can't encode: a drift finding on a
   claim whose meaning didn't change is an **annotation**, not a rewrite; only a
   section whose content actually changed is rewritten; when a change is
   genuinely ambiguous, **skip it and leave the finding open** rather than
   guess. For a source with no retrievable base version (mtime/web), a removal
   is **conflict-flagged** — present both sides, never auto-delete over an edit.

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
