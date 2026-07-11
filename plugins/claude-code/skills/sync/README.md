# /sync — bring a bound mem up to date with its source

`/sync <binding>` is the **sole maintenance writer** for a bound mem. It runs the
engine-rendered sync brief — what changed in the source since the last sync plus
any open verify findings, with the conservatism rules baked in — and applies only
the updates the brief calls for, inside the destination mem.

`/sync --verify <binding>` is the read-only measurement mode (absorbed from the
former standalone `/verify` skill, 2026-07-11): it runs `memstead projection
verify`, presents the engine's deterministic fidelity report verdict-first, and
frames a near-zero first report on an adopted mem as onboarding, not failure.
It mutates nothing; the findings it records are input for the next `/sync` run.

- Reads your source, writes your mem — never the reverse. Not a version-control
  operation.
- Conservative by default: a drift finding on an unchanged claim is an
  annotation, not a rewrite; an ambiguous change is skipped and left open; a
  removal over a non-retrievable base version (mtime/web) is conflict-flagged,
  never auto-applied.
- Records per-artifact dispositions via `memstead projection advance` so the
  sync baseline moves forward only over what was actually handled.
- Carries source-provenance `anchors` on its writes when the installed engine
  supports them (gated on the setup-recorded binary version); otherwise it
  proceeds without and says so — it never probes by sending anchors to see if
  they are rejected.

A refusal (sync not enabled on the binding, or an unsupported medium) carries the
one-command remedy (`memstead projection enable sync <binding>`), surfaced
verbatim.
