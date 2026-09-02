---
type: decision
created_date: 2026-09-02T12:41:02Z
last_modified: 2026-09-02T12:56:37Z
status: accepted
decided_on: 2026-09-02
deciders: operator (backlog-engine bundle A, go of 2026-09-02), implementing agent
scope: subsystem
tags: push,mem-repo,hooks,ci
---

# memstead push --all publishes the whole mem-repo and the workspace pre-push hook carries it

## Decision
`memstead push --all` publishes every mounted git-branch mem's declared branch plus the mem-repo's schema-and-config ref, fast-forward only: one `ls-remote` decides which refs lag, a ref already at the remote's SHA is skipped silently, one line per ref moved, so a run with nothing to publish prints nothing and exits 0. A ref that cannot fast-forward is refused by name (`NON_FAST_FORWARD`, the mem named) while the other lagging refs still go, and the run exits non-zero at the end with every refused and pushed ref under `details`. `--force` stays on the single-mem verb and is refused beside `--all`; folder and archive mounts have no branch and are skipped. The workspace repo's pre-push hook runs it against the dogfood workspace whenever a pushed range touches that workspace's engine state directory, refuses naming the ref and the command on failure, never invokes the engine for a code-only push, and can be bypassed with `MEMSTEAD_SKIP_MEMS_PUSH=1`; `scripts/install-hooks.sh` rehearses hooks under `MEMSTEAD_HOOK_DRY_RUN=1` so arming never publishes.

## Context
Until 2026-09-02 the single-mem `push` verb had no route for the schema-and-config ref, and the graph-health CI went blind whenever a session forgot to push a mem branch (the evidence-engine bundle's executing sessions never reached the remote). A human step (remember to push each mem) was carrying what a machine gate can carry. Engine: `Engine::push_all` over two new git-branch ops-table primitives (`ls_remote`, `resolve_ref`); the MCP surface is unchanged.

## Consequences
A pushed workspace commit that touched the graph is accompanied by the mem branches it describes, or the push is refused with the exact command to run. The remote holds the schema-and-config ref, so a second machine reconstructs the workspace. Deleting a remote branch stays a human decision: `push --all` never deletes.

## Options

The hook refusing a lagging substrate and telling the human to run the command: rejected, it keeps a human step a machine gate can carry. `--force` under `--all`: rejected, a force over every branch at once is the fork-healing accident the two-machine decision recorded.
