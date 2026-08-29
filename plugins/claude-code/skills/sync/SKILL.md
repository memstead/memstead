---
name: sync
description: >
  Your source changed — bring the mem up to date. Reads what changed since the
  last run plus any open findings, and updates only the affected entities,
  conservatively. The single maintenance writer for bound mems; run `--all` on
  a loop to keep every bound mem current (the loop ends itself once every
  binding reports quiescence), `--verify <binding>` for a
  fidelity report (coverage, accuracy, freshness) that writes no entity but
  does record findings and a verified baseline, or `--inventory <binding>` for
  the on-demand full stock-take —
  measure the whole binding, repair to quiescence, report — or `--sweep <mem>`
  for the standing-claim walk: verify what the mem asserts even where no
  source-change signal points, entity by entity, section by section. Not a
  version-control operation: changes flow from your source into your mem,
  never the reverse.
allowed-tools: Bash, Read, mcp__memstead__memstead_schema, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_delete, mcp__memstead__memstead_check
argument-hint: "[--all | <binding> | --verify <binding> | --inventory <binding> | --sweep <mem>]"
---

# Memstead Sync

Bring a bound mem up to date with its source — the **sole maintenance
writer** — or measure it with `--verify`: no entity writes, but it does record
findings and a `#verified` baseline. Read-only on your source; refusals verbatim.

## Steps

1. `WS="$(node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" root "$(pwd)")"` —
   every `memstead` call below carries `--workspace "$WS"`, never bare cwd.
   Parse `$ARGUMENTS`: a binding id (`<mem>/<stem>`) → step 2. `--verify
   <binding>` → step 6. `--inventory <binding>` → step 7. `--sweep <mem>` →
   step 8. No argument? Ask.
   `--all` → run

   ```sh
   memstead --json --workspace "$WS" projection brief --all --operation any --consume
   ```

   (`--consume` takes the slot; gated like step 2's anchors via `gate "$(pwd)" consume`: not capable → drop it, say the `reason`.)

   No bindings configured → say so and stop. Nothing due → say so and stop —
   and under a recurring loop (a scheduler re-prompting this conversation), a
   second consecutive nothing-due rotation means the catch-up job is DONE:
   cancel the schedule driving the loop and report quiescence in one line (a
   standing watch is a deliberate restart at a slower cadence).
   Otherwise execute each returned brief faithfully per its named operation:
   verify measures and records findings, never entities; build grows the mem — a build brief IS
   the sanctioned backfill channel, so cover the batch it asks for rather
   than declining it as out-of-scope (steps 2 and 4 apply); sync maintains
   it (steps 2–5).

2. Check the anchors capability once:
   `node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"` —
   **capable** → include `anchors` naming the source artifact(s), with
   `source` = the brief-listed entry point; **not capable** → omit, say why.

3. Render and read the sync brief, then follow it — it carries the changed
   slice, the open findings, and the conservatism rules:

   ```sh
   memstead --workspace "$WS" projection brief --sync <binding>
   ```

4. Apply only what the brief calls for, via the MCP mutation tools, inside the
   destination mem only. A drift finding whose meaning didn't change is an
   **annotation**, not a rewrite; a genuinely ambiguous change is **skipped,
   finding left open**, never guessed; a removal with no retrievable base
   version is **conflict-flagged** — present both sides, never auto-delete.

   Three repair disciplines bind here and in step 7 (each earned its place by
   measurement — the drift benchmark's run series, 2026-08):

   - **Claim walk on drift, never a gestalt call.** A finding that flags a
     drifted or stale entity is worked claim by claim: enumerate the entity's
     checkable assertions (counts, names, versions, paths, enumerations,
     dates, behavioral claims) section by section and check each against the
     current source before re-baselining anything. "Reads fine overall" is
     the measured top miss cause — a re-baselined anchor consumes the drift
     signal and hides the stale claim from every later pass.
   - **Post-edit recheck.** After any entity edit, re-read the whole entity
     once against the source and fix what still contradicts it. Partial
     repair is invisible to the loop otherwise.
   - **Section-local reconciliation.** When one section of an entity
     contradicts another (a normative section asserting X while a dated
     note, correction, or the frontmatter records not-X), correct the
     normative text itself, dated, so a reader of that section alone gets
     the truth — a correction merely sitting next to stale normative text
     does not heal the entity. History stays history: a done/abandoned
     record that explicitly frames its stale text as a preserved dated
     record and routes to existing current truth is exempt.

5. Record what you did so the baseline advances:

   ```sh
   memstead --workspace "$WS" projection advance <binding> --dispositions '{"<artifact-id>":"worked", …}'
   ```

   Anchored writes count as worked on their own; supply dispositions only for
   the rest (skipped or out-of-intent artifacts), using only ids the brief
   listed. The baseline advances once the slice is fully dispositioned.

6. `--verify <binding>`: run `memstead --workspace "$WS" projection verify <binding>` —
   it records findings, never entities; it measures the mem, not your project's
   changes or tests. Present the engine's deterministic report as ordered —
   verdict and top actions first, never re-ranked. A near-zero first report on
   a pre-binding mem is onboarding — name the route (`/ingest`, then `/sync`).

7. `--inventory <binding>`: the full stock-take — measure completely, then
   repair to quiescence. Start with the complete measurement:

   ```sh
   memstead --workspace "$WS" projection verify <binding> --full
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

8. `--sweep <mem>`: the standing-claim walk — verify what the mem asserts even
   where no change signal points at it. Works on any mem, bound or not; this
   is the pass that reaches drift the briefs structurally cannot see (stale
   claims in unchanged entities, unbound mems, evidence outside the binding
   scope). For each entity, prioritized by load-bearing frontmatter first
   (active milestones with past target dates, done certifications, retired
   statuses over living prose), then entities longest unverified:

   - extract its checkable factual claims section by section and verify each
     against the workspace's own trees and the graph; correct what the
     evidence refutes, dated and conservative, via the MCP mutation tools;
   - apply the step-4 disciplines (claim walk, post-edit recheck,
     section-local reconciliation);
   - record the outcome as a check record (`memstead_check`, or
     `memstead check` on the CLI) — verified-clean and corrected entities
     alike, so the walk leaves a machine-readable trace and the next sweep
     can prioritize by staleness of that trace;
   - cheap wide nets first where the mem is large: grep the mem's backticked
     paths, UPPER_SNAKE codes, symbols, and command names against the live
     tree in bulk before walking entities one by one — measured to clear
     hundreds of identifiers for a handful of calls;
   - claims only an external system could confirm (a live deployment, a
     third-party product) are out of scope: note the limit in the check
     record's method, never guess.

   Never create or delete entities in a sweep; what has no covering entity is
   reported, not invented. Close with per-mem counts: entities walked,
   corrected, check-recorded, and the claims left unverifiable with reasons.

## Rules

- The **sole maintenance writer** for bound mems. Changes flow source → mem,
  never mem → source; this is not a version-control operation.
- Conservative by default — the brief's rules bind. When unsure, skip and leave
  the finding for a later pass: a stale finding is cheaper than a wrong edit.
