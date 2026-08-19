---
type: decision
created_date: 2026-08-19T07:02:24Z
last_modified: 2026-08-19T07:02:24Z
status: accepted
decided_on: 2026-08-19
deciders: backlog-sweep plan 06 (operator decisions 14-15, 2026-08-18, as amended)
scope: subsystem
tags: out-of-root, workspace-layout, hints, honesty
---

# Layout guidance fires at the layout decision and claims only what measurement supports

## Decision
Out-of-root sources stay a fully supported shape — no refusal anywhere — and the engine's guidance about them moves to where the layout is actually decided and says only what holds. `mem-repo init` inside a git repository emits a source-layout hint (the works/degrades split plus the common-parent recipe) at bootstrap time; `projection init`'s out-of-root warning keeps firing but now states what measurement shows: enumeration, change detection, and anchor resolution all work, while artifact ids render as `../…` chains and the workspace-to-source relative layout must stay fixed. The retired "anchors will orphan" claim is gone — the dogfood probe on a live out-of-root binding measured 0 of 110 anchors orphaned post-pointer-join. In the same act, `mem-repo init` with workspace == git-repo root appends `mem-repo/` to that repo's own `.gitignore` (the old parent-first walk skipped exactly this case) and names `.memstead/` as intentionally trackable.

## Context
The first external field use (WOENENN, 2026-08-18) hit the chain in one hour: the layout warning arrived only after `projection init` — three commands after the layout decision at `mem-repo init`; the `.gitignore` heuristic skipped the workspace-at-repo-root case where the append matters most; and the warning's central claim (orphaned anchors) had been falsified by [[anchor-artifact-paths-speak-the-source-dialect-and-write-time-resolves-or-refuses]]. Overstated warnings train dismissal.

## Consequences
A newcomer bootstrapping over sibling repos learns the trade-off at the moment they choose a layout, gets a clean `git status` afterward, and reads warnings that survive verification. The recipe (root the workspace at the sources' common parent) is documented in the getting-started guide. The warning text is now calibrated to measured behaviour — if the anchor semantics change again, the warning must be re-measured, not extrapolated.

## Relationships
- **REFERENCES**: [[anchor-artifact-paths-speak-the-source-dialect-and-write-time-resolves-or-refuses]]

## Options



## Notes


