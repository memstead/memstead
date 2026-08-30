---
type: decision
created_date: 2026-08-23T02:38:44Z
last_modified: 2026-08-30T00:24:19Z
status: accepted
decided_on: 2026-08-21
deciders: operator (reasoning-substrate bundle, confirmed 2026-08-23), implementing agent
scope: subsystem
tags: schema, signals, edge-load, thresholds, health-axis, reasoning-substrate
---

# Serve declared aggregate signals as counts with thresholds and evidence

## Decision
We chose to let a type declare aggregate signals the engine computes and serves: exact, parameter-free counts with declared thresholds, reported with their evidence, never scored, never blocking. One kind ships in this wave, `edge_load`: count edges of an inline relation set in a declared direction, optionally restricted to edges whose counterpart holds a declared enum value (`neighbour_field` / `neighbour_value`). Thresholds map counts to the new two-member `SignalLevel` enum (`notice` / `warn`), deliberately separate from `ConstraintSeverity`; below the first threshold the level is `none`. Values are computed at read time in O(degree), never stored, never metadata, never part of `_hash`. Three surfaces serve them: every entity read of a declaring type (`_signals` on the structured envelope, the frontmatter headline plus a `## Signals` contributors section on the text channel), the include-gated `signals` health axis (above-`none` entities with per-level counts; `warn` participates in strict mode, `notice` never), and the out-of-band `SIGNAL_THRESHOLD_CROSSED` warning on mutations that move a signal across a threshold in either direction, never error-shaped ([[engine--graph-health-report-surface]], [[engine--entity-read-projection-surface]]).

## Context
Fourth form of the reasoning-substrate wave and the operator's originating idea for the whole program: a schema declares statistical signals over the graph, the engine computes them, the agent reads them with the schema's prose telling it how. The 2026-08-21 research sharpened the buildable form and rejected the scoring families; the 2026-08-23 deep review moved `distinct_by` out (the engine has no per-edge provenance and `Actor` is a caller category, not an identity) and added the neighbour filter, without which the wave's headline honesty check (no open defeater on this claim) is not expressible.

## Consequences
- No scores, no weights, no arithmetic beyond counting; a signal may not reference another signal. The form-refusal principle binds: signals must never become a probabilistic substrate.
- Raw edge counts are gameable by one author repeating one objection; consumer schema prose says so, the engine does not pretend otherwise. The independence count is a named follow-up: an engine-owned per-edge provenance sidecar keyed (source, rel_type, target), then `distinct_by: mutation`, never `distinct_by: actor`.
- The crossing warning's candidate set is the write's edge endpoints plus the updated entity's counterparts; dry-run rehearsals move nothing and warn nothing.
- Part of the declared release wave; keys and the warning code ship with the close-out; until the tag no built-in, example, or scaffold emits them.
- Schemas without signals keep byte-identical responses on every surface, and the canonical markdown form (anchor hashing, export, parser round-trips) stays signal-free by contract.

## Relationships
- **INFORMED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **REFERENCES**: [[engine:graph-health-report-surface]]
- **REFERENCES**: [[engine:entity-read-projection-surface]]

## Options

- Iterative truth-discovery scoring (TruthFinder family): rejected, fixpoint results depend on initialization constants.
- Weighted or gradual argumentation semantics: rejected, parameter choices change rankings with no principled defense; Goodhart bait.
- A third `notice` member on ConstraintSeverity: rejected, twelve non-exhaustive comparison sites would silently treat it as warn, and the meta-schema would advertise a value the loader refuses almost everywhere. A signal level is the output of a threshold, not the severity of a violation.
- Storing computed levels into entity metadata: rejected, derived state in the authoritative store is the hybrid-that-rots pattern.
- Serving signals opt-in (`include_signals`): rejected, the schema author opted in by declaring; a reader who must ask for the signal is a reader who forgets to.
- `edge_ratio` in the same wave: deferred on demand, not rejected.

## Notes

Landed 2026-08-23 with reasoning-substrate plan 04; the keys and the SIGNAL_THRESHOLD_CROSSED code shipped with the wave as engine **0.10.0** (public CHANGELOG `## [0.10.0] - 2026-08-23`, whose forward-compatibility note names `signals`), so they no longer sit under [Unreleased]. The post-tag state of the deferral above is unchanged: no built-in schema or example declares `signals`; only the `memstead schema new` scaffold documents the key (verified 2026-08-30).
