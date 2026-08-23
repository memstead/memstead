---
type: decision
created_date: 2026-08-23T13:43:26Z
last_modified: 2026-08-23T13:59:40Z
status: accepted
decided_on: 2026-08-23
deciders: operator, implementing agent
scope: system
tags: graph, health, ci, gates, dogfood
---

# The dogfood graph is refereed against a declared residual list

## Decision
We will run the project's own knowledge graph through a referee, `scripts/graph-health.sh` in the workspace repo, that turns every defect into one stable finding line (`mount_unbacked:<mem>:<reason>`, `schema_pin_mismatch:<mem>`, `dangling_link:<from>-><target>`, `orphan_stub:<id>`, `anchors_drifted:flagship`, `claims_sweep:red`, and the rest) and compares the set against a declared residual list (the workspace repo's `expected-residuals.txt` under its fixtures folder), where every known-open finding carries the plan that closes it. The run is green only when the two sets are EQUAL: a new defect is red, and a residual that stopped occurring is red too, so the list shrinks as repairs land instead of rotting into an allowlist. The runner takes `health --strict` with the integrity, anchors, stale, required-outgoing, constraints and signals includes, the mount roster, the flagship `projection verify` under a drifted-anchor ceiling, and the launch-claims sweep; it replays recorded fixtures without an engine run, and it mutates nothing beyond the flagship `#verified` token that verify records (named in its help). A scheduled lane runs it daily, on dispatch, and on pushes touching the graph's inputs; until the mem-repo deploy key exists it reports a named skip in the run summary and exits green (bundle decision K).

## Context
No workflow, hook or cron ran `memstead` against the dogfood workspace. The graph spans two repositories and a third clone (six branch mems in the private `memstead/mems-repo`, the `project` and flagship folder mems in the workspace repo, `engineering` in the engine submodule), so no single repo's CI saw it, and absence-of-activity drift needs a clock, not a commit hook. The unrepaired state on 2026-08-23, recorded in the workspace repo's pre-repair fixture with the engine that can see it: 28 findings (two unbacked mounts, three pin mismatches, two rotted schemas, fourteen dangling links, five orphan stubs, 36 drifted flagship anchors, a red claims sweep). The repair plans that follow need a referee that bites before they start and a list that says exactly what they are expected to close. See [[engineering--health-strict-refuses-configuration-defects-and-a-mount-that-resolves-to-nothing-says-so]] for the engine half.

## Consequences
- The repair plans own named residual lines and are done when their lines are removed and the run stays green; a plan that fixes something nobody declared shows up as a stale residual, which is also red, so the list is always exact.
- A defect introduced by any session anywhere in the three repos is red on the next push or by the next morning.
- The lane cannot run until the operator sets `MEMS_REPO_DEPLOY_KEY` (a read-only deploy key on the private mem-repo); until then every run is a named skip in the summary, never a pass and never a red.
- The flagship verify inside the runner advances the `#verified` token in the flagship sidecar; locally that is an operator-committed file, in CI it dies with the checkout.
- Cost: one engine build per lane run (cached), and the residual file must be edited by the plan that repairs, which is the intended friction.
- The recorded fixture is the proof the gate bites: its replay against an empty residual list is red on every class, its replay against the declared list is green, and a clean fixture is green.

## Relationships
- **REFERENCES**: [[health-strict-refuses-configuration-defects-and-a-mount-that-resolves-to-nothing-says-so]]

## Options

- A plugin Stop hook or a pre-push hook as the graph gate: rejected. Drift by absence of activity needs a clock, and no single repo's push sees the whole graph.
- A red lane until the secret exists: rejected (decision K). A red run for a missing secret teaches everyone to ignore red.
- Allowlisting the known defects as silent exceptions: rejected. An allowlist never shrinks; set equality makes a repaired line red until it is removed.
- Proving the red run by dispatching the lane against the pre-repair commit: rejected. The branch mems' state is in no workspace-repo commit; the recorded fixture is the replayable evidence.
- A referee on set equality with owned residuals, a recorded fixture, a skip-by-name lane: chosen.

## Notes

Fixtures and tests: the workspace repo's pre-repair fixture (the 2026-08-23 recording) and its `expected-residuals.txt` (28 lines, owners plan 03, 08 and 09), `scripts/graph-health.test.mjs` (replays: red on every class against an empty list, green against the declared list, a clean fixture green, a stale residual red, the anchor ceiling, the lane file's shape). The dispatch run of the lane after the secret is the terminal plan's.
