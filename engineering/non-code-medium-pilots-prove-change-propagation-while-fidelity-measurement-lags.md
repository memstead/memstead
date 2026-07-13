---
type: memo
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:43:05Z
status: active
tags: projection, verify, sync, pilot, evidence, s1b, engine
---

# Non-code-medium pilots prove change-propagation while fidelity measurement lags

## Claim
The S1b non-code-medium pilots proved the projection pipeline's **change-propagation** path works end-to-end over both non-git mediums, but its **fidelity-measurement** path (verify/prune) is incomplete for them — shipping `/sync` + `/verify` is sound for code sources today; graph and filesystem fidelity is recorded backlog, not production-ready.

## Context
The S1b milestone required two non-code bindings exercised build→sync→verify before sign-off, because every dogfood binding had been codebase-shaped. Run 2026-07-11: a **Graph-medium** binding (sacrificial two-mem workspace) and a **Filesystem/mtime** binding (an internal dogfood binding whose medium was re-typed codebase→filesystem+mtime). Anchors the [[engine--projection-verify-and-findings-store]] work.

## Relationships
- **REFERENCES**: [[engine:projection-verify-and-findings-store]]

## Substance

**Works:** both non-git change-detection strategies drove a real source change end-to-end — the Graph medium's graph-snapshot token and the Filesystem medium's mtime stat-map each surfaced exactly the changed artifact in the sync brief and were consumed by `projection advance`. Filesystem verify enumerated a real S(D)=106 and rendered grain-classed coverage + the capability-matrix block, correctly framing a mem-predates-binding run as onboarding (0% expected), not failure.

**Gap resolved (2026-07-11):** Filesystem conflict-flag now surfaces — base-retrievability is keyed on the resolved strategy, not the static medium type, so a filesystem+mtime binding renders `base_version_retrievable: false` and the `base-version-unretrievable → prune degrades to conflict-flagging` degradation (was `Degradations: (none)`).

**Gap still open (backlogged):** Graph-medium verify fidelity is inert — `entity`-namespace anchors are never resolved against the live source graph (always "unobserved"), source enumeration is unwired despite `enumerable: true` ("No S(D) denominator"), so coverage/anchor-resolution/**drift** are all `0/0` — a deliberately stale anchor over a changed source went unflagged. Also: engine writes untracked `.memstead/state/{findings,advance}/` into a tracked workspace; mtime baselines aren't machine-portable; no CLI medium-edit command exists (UniFFI/CLI parity gap).

## Alternatives



## Outcome

`/sync` + `/verify` ship (S1b). One of the two non-code-medium verify gaps is now **closed**: base-retrievability / the conflict-flag degradation derives from the resolved change-detection strategy (fixed 2026-07-11 — `FacetCapability::from_caps` AND-s the medium's static ceiling with `strategy_retrieves_base`; only `git`/`graph` hold a base leg, `mtime`/`none` degrade to conflict-flagging), verified live on that internal dogfood binding. The remaining gap — Graph-medium verify fidelity (source enumeration + entity-anchor resolution) — stays recorded in the internal backlog as a follow-up.
