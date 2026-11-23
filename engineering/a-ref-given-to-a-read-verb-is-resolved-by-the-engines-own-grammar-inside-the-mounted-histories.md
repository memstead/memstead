---
type: decision
created_date: 2026-11-22T22:11:51Z
last_modified: 2026-11-22T22:21:20Z
status: accepted
decided_on: 2026-11-22
deciders: operator (stuck-mem bundle, go of 2026-11-22), implementing agent
scope: system
tags: security,git-branch,diff,changes,mounts
---

# A ref given to a read verb is resolved by the engine's own grammar, inside the mounted histories

## Decision
We chose to resolve a ref or cursor that a caller hands to a read verb by a small grammar of the engine's own, against the histories of the mems the workspace mounts and nothing else. A ref is anchored at the branch of a mem in scope, at `HEAD`, or at a commit hash looked up among the commits in scope, and may step to parents with `~n` and `^n`. One value is accepted outside the grammar: the hash of the empty tree, which stands for the state before any commit and is how a caller asks for everything. The change feed and the commit-note reader take the named mem's own history as scope; the diff takes every mem the workspace mounts in the git directory. Every other ref refuses exactly as a ref that names nothing does, with the same payload and the same work done before the answer. No general revision parser sees a caller's ref, and no object outside the histories in scope is read.

## Context
Several mems lie as branches in one git directory, and a workspace may mount some and not others. Every read verb and every write kept the mount roster as the boundary, except the two that take a ref: they handed it to a general revision parser, which resolves anything the directory holds. A caller in a workspace that mounted one mem could read another's entity text through a diff and its entity ids through the change feed. The first fix judged a ref by the commit it resolved to; independent graders broke it by a form rooted at an unmounted fork, by latency, by syntax that reads outside objects while it resolves, and by a branch spelled like a hash.

## Consequences
- The mount roster is the read boundary for refs as for everything else: what a workspace does not mount does not exist for its callers, not even by the name of its branch.
- The anchor counts as well as the commit: a form rooted at an unmounted fork refuses although it walks into mounted history.
- Tags, remote-tracking refs and every revision syntax beyond parent steps are not read; a caller that used them is refused.
- A cursor kept from a history that was replaced refuses like an unknown one, and the caller starts again from the empty tree.
- A hash is found by walking the history in scope, so its cost follows the length of that history.

## Relationships
- **DERIVED_FROM**: [[a-mem-that-exists-as-a-branch-is-mounted-by-a-cli-verb-that-writes-a-roster-row]]

## Options

- A refusal code of its own for a ref outside the scope: rejected, it would tell that the ref exists.
- Judge the ref by the commit it resolves to, whatever its spelling: tried and rejected, a form rooted at a fork tells that the fork exists.
- Keep the general parser and filter its result: rejected, it reads outside objects to resolve and its latency differs.
- Restrict the diff to the named mem's own history: rejected, the diff between mounted peers is documented and both are readable anyway.
- An own grammar of anchors and parent steps over the histories in scope: chosen.
