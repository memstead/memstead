---
type: decision
created_date: 2026-09-05T18:25:09Z
last_modified: 2026-09-08T21:08:57Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: claims, anchors, preparation, verify, binding
---

# A public claim anchors the words that assert it: the claims register is a verified binding under the quoted-phrase preparation

## Decision
The public-claim register (`exec-launch-claims`) is a binding whose only operation is verify. Each claim anchors the surface that asserts it and the artifact that backs it, and the engine gained the generic pieces this needs. A `quoted-phrase` preparation: under a source declaring it, the artifact `<path>#<phrase>`, `<url>#<phrase>` or `<entity-id>#<phrase>` addresses the occurrence of a literal phrase, its prepared form is the phrase itself, so the anchor resolves while the text still carries the words and reads orphaned once they leave, whatever else changed around them; a url row's supplied observation is re-prepared under the same rule, so a page is adjudicated on its phrase and not on its bytes. A `file` anchor of class `authored` is the existence-only check the old `tracked` directive was. Archived surfaces (the launch kit in the attic) are history, not anchors: a claim whose every asserting surface is archived, and a withdrawn claim, carry no anchor and are declared entity exclusions with their reason (`projection exclude --entity-exclusions`), which the fidelity report's new entity side of coverage names as excluded rather than as owed. Live pages enter as observations: the graph-health lane lists the register's url anchors through `memstead anchors --mem <mem> --grain url`, fetches each with curl and hands the text to `verify-anchors --observations`; the engine never fetches, and a source scan in the base crate pins that. A forbidden-wording rule (the old `forbid` directive) has no engine negative and stays as prose in the claim's evidence, read by the grader.

## Context
The claims sweep (`scripts/claims-sweep.py`, 364 lines) re-implemented anchor adjudication with its own directive grammar (`surface` / `forbid` / `file` / `contains` / `tracked`, 54 checks over 18 claims) inside claim bodies, a second contract beside the engine's anchors, and it went red on 2026-09-04 when a directive named a deleted file. The engine's anchors already carried `span`, `file`, `url` and `entity` grains, supplied observations and per-anchor drift adjudication; what they lacked was a way to say "this text still carries these words" (a span hashed its whole file, so any rewrite drifted), a way to read a mem's url roster, and a way to name a destination entity that deliberately anchors nothing. Two defects surfaced on the way and are fixed: the fidelity report's population test matched the artifact string with its locator attached, so every located span and every url row was silently out of scope; and no surface listed one mem's anchors.

## Consequences
The sweep's 54 directives became 31 anchor rows (28 anchored, 3 existence-only) on 16 claims plus 2 declared entity exclusions; `projection verify exec-launch-claims/claims --full --fail-on-findings --fail-on-inconclusive` is the referee, clean on the tip, red when a surface loses its phrase, inconclusive when the observation step is skipped on a fresh sidecar. The claims binding's exclusion ledger is versioned (the advance store's ignore rule carries one negation) because it holds authored decisions the CI verify must read. A recorded url observation ages visibly on every anchor surface but does not expire; whether an observation should have a shelf life is an open question, not decided here. The sweep script itself retires with the bundle's terminal plan.

## Relationships
- **GOVERNS**: [[engine:preparation-registry]]
- **MOTIVATED_BY**: [[a-freshness-claim-is-opt-in-measurement-machinery-is-not]]
