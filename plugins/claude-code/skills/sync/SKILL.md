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
writer** — or measure it with `--verify` (findings and a `#verified`
baseline, no entity writes). Source read-only; refusals verbatim.

## Steps

1. `WS="$(node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" root "$(pwd)")"` —
   every `memstead` call below carries `--workspace "$WS"`, never bare cwd.
   Parse `$ARGUMENTS`: a binding id (`<mem>/<stem>`) → step 2. `--verify
   <binding>` → step 6. `--inventory <binding>` → step 7. `--sweep <mem>` →
   step 8. No argument? Ask. `--all` → run
   `memstead --json --workspace "$WS" projection brief --all --operation any --consume`
   (`--consume` takes the slot; gated like step 2's anchors via
   `gate "$(pwd)" consume`: not capable → drop it, say the `reason`).
   No bindings configured → say so and stop. Nothing due → say so and stop;
   under a recurring loop, a second consecutive nothing-due rotation means
   the catch-up job is DONE — cancel the schedule, report quiescence in one
   line (a standing watch is a deliberate restart at a slower cadence).
   Otherwise execute each brief per its named operation: verify measures and
   records findings, never entities; build grows the mem (the sanctioned
   backfill channel — cover the batch it asks for; steps 2 and 4 apply);
   sync maintains it (steps 2–5).

2. Check the anchors capability once:
   `node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" gate "$(pwd)"` —
   **capable** → include `anchors` naming the source artifact(s), `source` =
   the brief-listed entry point; **not capable** → omit, say why.

3. Render and read the sync brief, then follow it — it carries the changed
   slice, the open findings, and the conservatism rules:
   `memstead --workspace "$WS" projection brief --sync <binding>`.

4. Apply only what the brief calls for, via the MCP mutation tools, inside
   the destination mem only. An unchanged-meaning drift finding is an
   **annotation**, not a rewrite; a genuinely ambiguous change is **skipped,
   finding left open**, never guessed; a removal with no retrievable base is
   **conflict-flagged** — present both sides, never auto-delete. Three
   measured disciplines bind here and in steps 7–8: **claim walk on drift**
   (check a flagged entity assertion by assertion against the source before
   any re-baseline — never a gestalt "reads fine", the measured top miss
   cause); **post-edit recheck** (re-read the whole edited entity once
   against the source); **section-local reconciliation** (when an entity's
   sections disagree, correct the stale normative text itself, dated — a
   correction merely beside it does not heal it; an explicit dated
   historical record routing to existing current truth is exempt).

5. Record what you did so the baseline advances:
   `memstead --workspace "$WS" projection advance <binding> --dispositions '{"<artifact-id>":"worked", …}'`.
   Anchored writes count as worked on their own; supply dispositions only
   for the rest (skipped or out-of-intent), using only ids the brief
   listed. The baseline advances once the slice is fully dispositioned.

6. `--verify <binding>`: run `memstead --workspace "$WS" projection verify <binding>` —
   findings, never entities; it measures the mem, not your project's
   changes. Present the engine's report as ordered, verdict first, never
   re-ranked. A near-zero first report is onboarding — name the route
   (`/ingest`, then `/sync`).

7. `--inventory <binding>`: the full stock-take. Measure completely with
   `memstead --workspace "$WS" projection verify <binding> --full`, then
   repair in passes (steps 2–5 off the sync brief, re-verify, repeat). Done
   when the brief has nothing to sync AND the re-verify is clean or every
   remaining finding carries a disposition. **Hard rule — monotone
   progress:** count open work before each pass (open findings + artifacts
   awaiting disposition); a pass that does not strictly shrink it ends the
   run with an honest "did not converge" naming the stuck items — never a
   silent loop. Keep no state between passes (the engine's dispositions are
   the resume point); close with the step-6 report, verdict first.

8. `--sweep <mem>`: the standing-claim walk — verify what the mem asserts
   even where no change signal points, on any mem, bound or not. Walk
   load-bearing frontmatter first (past target dates, done certifications,
   retired statuses over living prose), then longest-unverified. Per
   entity: verify its checkable claims against the workspace's trees and
   graph under the step-4 disciplines, correct what the evidence refutes,
   and record a `memstead_check` verdict — clean and corrected alike — so
   the next sweep prioritizes by trace staleness. Bulk-grep the mem's
   paths, codes, and symbols against the tree first. Externally
   unconfirmable claims are out of scope — note the limit, never guess.
   Never create or delete entities in a sweep — the uncovered is
   reported, not invented — and close with per-mem counts.

## Rules

- The **sole maintenance writer** for bound mems. Changes flow source → mem,
  never the reverse; not a version-control operation. Conservative by
  default — the brief's rules bind; when unsure, skip and leave the finding
  open: a stale finding is cheaper than a wrong edit.
