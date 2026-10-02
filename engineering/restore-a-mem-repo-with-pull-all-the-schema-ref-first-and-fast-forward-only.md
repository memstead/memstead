---
type: decision
created_date: 2026-10-02T21:18:37Z
last_modified: 2026-10-02T22:03:28Z
status: accepted
decided_on: 2026-10-02
deciders: operator, implementing agent
scope: subsystem
tags: transport, mem-repo, restore, multi-machine
---

# Restore a mem-repo with pull --all, the schema ref first and fast-forward only

## Decision
We will restore a mem-repo from its remote with one CLI verb, `memstead pull --all`, the inverse of `push --all`. It fetches once per mem-repo, fast-forwards the schema-and-config ref first, then re-reads the schema catalogue, re-resolves every mounted mem's pin from its config and re-attaches every mem quarantined `SCHEMA_NOT_FOUND`, then fast-forwards each mounted mem's declared branch through the same whole-tree schema gate `pull` runs, and last imports the mounted mems' check rows. A ref absent locally is created; a ref with local commits the remote lacks is refused `LOCAL_DIVERGENCE` by name while the others still move. Mems on the remote that nothing mounts are listed with their mount command (`mem init <name> --schema <pin> --reattach`), never mounted by the run. It is the one route that moves the local schema-and-config ref from a remote: `fetch` refuses a refspec whose destination is a local branch. `mem-repo init --remote <url>` configures `origin` at bootstrap, so a workspace cloned with its tracked engine state restores in two commands.

## Context
On 2026-10-02 the dogfood mem-repo lagged its remote by 1,118 commits on the schema-and-config ref. The per-mem `pull` refused the engine mem `SCHEMA_VIOLATION_IN_FETCH` because its incoming content used a field of a schema generation only the remote's ref carried, and the flagship mem sat quarantined for the same reason. No verb moved that ref from a remote, so `status --remote`'s own remedy could not complete and the operator had to authorize a one-time raw git fast-forward against the rule that only the engine mutates a mem-repo. The same day a workspace cloned onto a second machine had only remote-tracking refs and could not be brought up by engine commands at all. The public guide's recovery worked only because its example pinned a built-in schema, and its per-mem `mem init` step forked the schema-and-config ref.

## Consequences
- A `push --all` backup is restorable by engine commands alone, on a cloned workspace (`mem-repo init --remote <url>`, `pull --all`) and from nothing but the backup (plus one `mem init --reattach` per mem).
- A pin switched on another clone governs validation here after the pull, instead of refusing the content written under it.
- `status --remote` names `pull --all` as the reconcile; the handbook, the plugin's sync skill and the backup guide follow.
- A `fetch` refspec into a local branch, which used to pass to git unchanged, now refuses `INVALID_INPUT`.
- Cost: one extra `ls-remote` per mem-repo to name unmounted mems; the run re-resolves every mounted mem's pin after the schema ref moves.
- Not covered: divergence. A forked ref is refused and stays a human decision; the entity-wise merging pull stays its own work.

## Relationships
- **MOTIVATED_BY**: [[memstead-push-all-publishes-the-whole-mem-repo-and-the-workspace-pre-push-hook-carries-it]]
- **INFORMED_BY**: [[remote-staleness-is-an-axis-of-memstead-status-read-only-and-fail-open]]
- **IMPLEMENTS**: [[engine:git-transport-and-history-surface]]

## Options

- Divergence-aware entity-wise merge on pull: rejected for restore, it answers both-sides-wrote, a different problem.
- Let the single-mem `pull` also move the schema-and-config ref: rejected, one mem's pull would move every other mem's pin as a side effect.
- A pseudo-mem name for the schema ref (`pull <ref>`): rejected, it puts an internal ref name on the user surface.
- Rebuild the mount roster from the remote's configs: rejected, membership would change silently; the run lists unmounted mems with their mount command instead.
- Keep a documented raw-git workaround: rejected, it contradicts engine ownership of the mem-repo.
- An MCP tool for the restore: rejected, moving a mem-repo is an owner decision at the terminal, the posture transport already has.
- Chosen: `pull --all` with the schema ref first, fast-forward only, CLI only.

## Notes

The bootstrap seed of the schema-and-config ref is deterministic (fixed signature and time), so a freshly initialised mem-repo fast-forwards onto any backup made from a workspace bootstrapped the same way.
2026-10-02 amendment: the first independent grade refuted the restore of backups bootstrapped by an earlier engine (their schema-ref root differs from today's seed). The schema ref therefore follows one narrow extra rule: a local schema ref that is nothing but a root commit over the empty tree carries no state and gives way to the backup's history; a schema ref with any state is still refused `LOCAL_DIVERGENCE`. In the same pass a ref that only holds unpushed local work became a report, not a refusal; a mem-repo that cannot be fetched is named while the others restore; `mem-repo init --remote` on an existing mem-repo re-points the remote so a restore retries with the documented command; and every transport call to git ends option parsing before the remote and refspecs, after the grade showed a refspec could run a command through `fetch`.
2026-10-03 amendment: the second grade showed the fetch guard, written as a denial of `refs/heads/`, let `refs/HEADS/x` through on a case-insensitive filesystem. The guard is now an allowlist: a fetch refspec's destination must start with `refs/remotes/`, anything else refuses `INVALID_INPUT`. In the same pass connection failures (unknown host, refused port) classify as `UNKNOWN_REMOTE` instead of an untyped failure, and `mem-repo init --remote` refuses a value starting with `-`. The third, final grade confirmed all six criteria of the plan.
