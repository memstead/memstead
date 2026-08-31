---
type: memo
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-08-31T16:25:05Z
status: active
tags: projection, verify, sync, pilot, evidence, s1b, engine
---

# Non-code-medium pilots prove change-propagation while fidelity measurement lags

## Claim
The S1b non-code-medium pilots proved the projection pipeline's **change-propagation** path works end-to-end over both non-git mediums, while its **fidelity-measurement** path (verify/prune) lagged behind for them. That lag is now closed for both: filesystem base-retrievability resolved 2026-07-11, graph enumeration and entity-anchor resolution 2026-08-21. The claim this memo was written to record — that change-propagation ran ahead of measurement — is history rather than current state; it is kept because the *shape* recurs: a capability row can declare what no code delivers, and only a pilot that tries to use it finds out.

## Context
The S1b milestone required two non-code bindings exercised build→sync→verify before sign-off, because every dogfood binding had been codebase-shaped. Run 2026-07-11: a **Graph-medium** binding (sacrificial two-mem workspace) and a **Filesystem/mtime** binding (an internal dogfood binding whose medium was re-typed codebase→filesystem+mtime). Anchors the [[engine--projection-verify-and-findings-store]] work.

## Relationships
- **REFERENCES**: [[engine:projection-verify-and-findings-store]]
- **INFORMED_BY**: [[issue-trackers-enter-as-a-memstead-owned-pilot-mirror-not-a-field-script-or-a-forge-medium]]

## Substance

**Works:** both non-git change-detection strategies drove a real source change end-to-end — the Graph medium's graph-snapshot token and the Filesystem medium's mtime stat-map each surfaced exactly the changed artifact in the sync brief and were consumed by `projection advance`. Filesystem verify enumerated a real S(D)=106 and rendered grain-classed coverage + the capability-matrix block, correctly framing a mem-predates-binding run as onboarding (0% expected), not failure.

**Gap resolved (2026-07-11):** Filesystem conflict-flag now surfaces — base-retrievability is keyed on the resolved strategy, not the static medium type, so a filesystem+mtime binding renders `base_version_retrievable: false` and the `base-version-unretrievable → prune degrades to conflict-flagging` degradation (was `Degradations: (none)`).

**Gap resolved (2026-08-21):** Graph-medium verify fidelity was inert — `entity`-namespace anchors were never resolved against the live source graph (always "unobserved"), source enumeration was unwired despite `enumerable: true` ("No S(D) denominator"), so coverage/anchor-resolution/**drift** all read `0/0` and a deliberately stale anchor over a changed source went unflagged. Both halves are now implemented: a graph source enumerates its source mem's in-scope entities as a real denominator, and entity anchors resolve against the live graph — the pilot's stale anchor now reports `drifted`. See the Outcome below for the design choices. **Still open (rechecked 2026-08-30):** mtime baselines aren't machine-portable. The other two items previously listed here no longer hold, and the list is corrected rather than left standing. (a) The engine's `.memstead/state/findings/` and `.memstead/state/advance/` writes are **tracked**, not untracked: `git ls-files` in the dogfood workspace lists `.memstead/state/advance/engine/graph.json` and `.memstead/state/findings/engine/graph.json`, and neither path is gitignored. (b) The CLI edit gap is closed: `memstead projection edit` is the general patch surface over the shared `pipeline_edit` layer, and a patch replacing the whole `sources` block covers the medium half (`type` / `pointer` / `change_detection`) that the retired standalone medium records used to hold, so there is nothing left for a separate medium-edit command to reach. (The former UniFFI half of that parity retired with the macOS app 2026-08-18.)

## Alternatives



## Outcome

`/sync` + `/verify` ship (S1b). One of the two non-code-medium verify gaps is now **closed**: base-retrievability / the conflict-flag degradation derives from the resolved change-detection strategy (fixed 2026-07-11 — `FacetCapability::from_caps` AND-s the medium's static ceiling with `strategy_retrieves_base`; only `git`/`graph` hold a base leg, `mtime`/`none` degrade to conflict-flagging), verified live on that internal dogfood binding. The remaining gap — Graph-medium verify fidelity (source enumeration + entity-anchor resolution) — was closed 2026-08-21; see the update below.


**Update (2026-08-21) — the remaining gap is closed in the engine.** Graph-medium verify now measures: a graph source enumerates its source mem's in-scope entities as a real `S(D)`, and `entity`-grain anchors resolve against the live graph — the pilot's exact failure (a stale-pinned anchor over a changed source entity going unflagged) is now a passing regression test that reports `drifted`.

Three design choices carried the fix, each general rather than graph-specific:

- **Scope is medium-shaped, and enforced or refused — never decorative.** A graph facet's scope is an entity-selector vocabulary (`*`, `type:<entity_type>`, `id:<glob>`), interpreted at run time and refused at binding validation when it is anything else. The prior state was worse than missing: `projection init` scaffolded the path glob `**/*` onto graph facets, which nothing anywhere interpreted — scope that looks like selection and reaches nothing.
- **Entity observation goes through the store and the canonical rendered form, not the filesystem.** A git-branch mem has no working-tree file to stat, so a path-based observation would have worked for folder mems and silently failed for others. Hashing the canonical render makes an anchor's recorded hash mean the same thing in both namespaces.
- **One enumeration entry point for every medium.** The bail was not one bug but five copies of the same loop — report, findings twice, the refinement rotation, and the exclude membership gate — so teaching only the report about a medium would have left the other four empty-handed. `git` sources were excluded from that same walk for the same reason and enumerate now too.

Standing guard against the class: `verify --full` refuses when a matrix-enumerable medium's walk yields nothing, so no future medium can be declared enumerable without an enumeration arm and still report green.
