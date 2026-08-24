---
type: decision
created_date: 2026-08-23T12:44:07Z
last_modified: 2026-08-23T12:44:07Z
status: accepted
decided_on: 2026-08-23
deciders: operator, implementing agent
scope: system
tags: release, gitlink, submodule, git-hooks, gates
---

# A gitlink moves only forward: ancestry refused, reachability reported

## Decision
We will let a staged submodule pointer move only to a descendant of the committed one, refused otherwise at commit time by a versioned `pre-commit` hook, and we will treat a pointer that is not yet on the submodule's remote as a report, never a refusal. The two arms are separated on purpose: a rewind is never an accident worth keeping, while an unreachable pin is the normal state between committing the gitlink and pushing the submodule (the workspace commits the pointer first, by design). Each arm has one named escape that prints a notice: `MEMSTEAD_ALLOW_GITLINK_REWIND=1` bypasses the ancestry arm, `MEMSTEAD_GITLINK_OFFLINE=1` skips the remote probe. The same posture governs the deploy workflow (a pin on the remote that diverges from `main` fails the deploy; an unpushed or lagging pin warns) and the adoption script (a tag that does not descend from the current pin is refused). The hooks are generic: every staged gitlink is checked the same way, no repository or path is named, and a `post-merge` hook moves exactly the submodules a pull moved so the checkout matches the pin afterwards.

## Context
On 2026-08-22/23 the `public` gitlink moved from `a50a418` to `88474a0` in an ordinary commit. The new commit was not a descendant of the old one: the submodule checkout had silently switched to a divergent line after a `git pull`, 16 commits dropped out of the pinned history, one unrelated commit came in, and 0.10.0 was first cut on that tree (merge-base `965c386`). Nothing refused because nothing looked: `core.hooksPath` was unset in both clones, the outer repo had no hooks at all, the deploy workflow could warn about a lagging pin but could not see a divergent one, and the adoption script verified the tag locally after a fetch, which a tag created and never pushed also passes. The `--locked` consumer builds catch version drift, not history drift. See [[engineering--the-tag-is-the-entire-outward-release]] for why a pin on the wrong line is a release on the wrong line.

## Consequences
- A backwards or sideways gitlink move is refused where it happens, with the two short shas named and the escape spelled out; recovery is a checkout, not an archaeology session.
- An unreachable pin is named at commit time (push the submodule first) but never blocks, so the commit-then-push order the workspace relies on keeps working.
- `deploy-branches.yml` now has three arms (`scripts/check-gitlink-pin.sh`): divergent fails, unpushed warns, lagging warns with the count. Production keeps lagging `main` deliberately.
- `scripts/adopt-release.sh` verifies the tag on the remote with `git ls-remote` and refuses a pin that does not descend from the current one.
- Hooks exist only when armed: `scripts/install-hooks.sh` sets `core.hooksPath` in both clones, dry-runs every refusing hook against the current tree first (a failing hook is not armed, and the script says which), and leaves a different pre-existing value untouched. The adoption runs it, `DEVELOPING.md` names it in the setup.
- Cost: a commit that stages a gitlink now probes the submodule's remote (one fetch); the offline escape exists for that. A deliberate rewind needs the escape variable, which is the point.
- Not covered: a submodule pointer moved by a tool that bypasses hooks (`--no-verify`) or from a clone where the hooks were never armed; CI's divergence arm is the backstop for those.

## Relationships
- **REFERENCES**: [[the-tag-is-the-entire-outward-release]]

## Options

- Refuse an unreachable pin as well: rejected. The workspace commits the gitlink before `public/` is pushed by design; refusing would break the normal order on every release.
- Fail the deploy on a lagging or unpushed pin: rejected. Production lags `main` deliberately, and an unpushed pin is the expected state before the operator's push.
- Detect drift in CI only: rejected as the sole gate. The 2026-08-22 move was already committed and pushed before any CI could look; the commit boundary is the only place a refusal prevents the history loss.
- Arm hooks by hand per clone (the previous instruction): rejected. It was documented for six weeks and never done; an install script that the adoption runs is a gate that arms itself.
- Refuse at commit time, report reachability, escape by environment variable: chosen.

## Notes

Exercised against fixtures in `scripts/release-machinery.test.mjs` (the replayed `a50a418 -> 88474a0` move against the real engine history, a forward move, an unpushed commit, both escapes, a fabricated pull, the four pin arms, both adoption refusals). Fixture runs are the evidence that each gate bites; none of them touched `main`.
