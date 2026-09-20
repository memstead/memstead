---
title: Back up a mem-repo to a remote
description: "Push a workspace's mems to any git remote and recover them on another machine — fetch, pull, push, and branch-reset end to end."
sidebar:
  order: 6
---

A mem-repo workspace keeps every mem as a branch in one local git
repository that the engine owns. That repository can be pushed to any
git remote — a bare repo on a NAS, a private GitHub repository, a USB
stick — and recovered from it, using only `memstead` commands. This
guide walks a full backup and recovery cycle.

Transport is deliberately a CLI-only surface: agents connected over MCP
have no push/fetch primitive. Moving a mem-repo off-machine is an owner
decision, made at the terminal.

All four commands need the full build (the default `memstead` binary)
and a git-branch-backed workspace (`memstead mem-repo init`).

## 1. Configure the remote

Point a named remote at wherever the backup should live. Any URL git
accepts works — SSH, HTTPS, or a local path:

```sh
# a private GitHub repo…
memstead mem-repo remote-add origin git@github.com:you/mem-backup.git
# …or a bare repository on any disk
git init --bare /backups/mems.git        # one-time, outside the workspace
memstead mem-repo remote-add origin /backups/mems.git
```

`remote-add` is an upsert: re-running it with a new URL re-points the
remote. Transport commands default to `origin`; pass `--remote <name>`
to use another.

## 2. Push

Each mem is one branch; push the ones you want backed up:

```sh
memstead push knowledge
```

The argument is the mem's name; the pushed ref is the branch the mem's
mount declares. The two usually coincide (`knowledge` lives on
`refs/heads/knowledge`), but a mount may declare any branch it likes —
a mem named `engine` on `refs/heads/team/engine` pushes that declared
branch, and the recovery flow below fetches and resets against it
(`refs/remotes/origin/team/engine`).

The push refuses rather than surprises:

- `UNKNOWN_REMOTE` — the remote name isn't configured (step 1).
- `NON_FAST_FORWARD` — the remote has commits this workspace lacks
  (another machine pushed). Run `memstead pull <mem>` first, or
  `--force` to overwrite deliberately (force-with-lease under the hood).
- `LOCAL_INVALID_STATE` — the local branch fails schema validation;
  nothing is sent. Fix the named entities first.

## 3. Recover on a fresh machine

The recovery flow bootstraps a workspace, then replaces each mem's
initial branch state with the backed-up one:

```sh
mkdir restored && cd restored
memstead mem-repo init
memstead workspace allow-create --schema '*' knowledge
memstead mem init knowledge --schema default@1.3.0
memstead mem-repo remote-add origin git@github.com:you/mem-backup.git

memstead fetch knowledge                                  # refs arrive, nothing moves
memstead branch-reset knowledge refs/remotes/origin/knowledge
memstead reload --mem knowledge
memstead status                                          # your entities are back
```

`branch-reset` is the deliberate "adopt the remote state" move: the
freshly-initialised local branch shares no history with the backup, so a
plain `pull` would refuse with `LOCAL_DIVERGENCE`. The reset discards
only the local initial commit — and it refuses
(`PUSHED_COMMITS_PROTECTED`) if it would ever discard commits that any
remote already has.

If the mem was published as an archive instead, `memstead install` is
the simpler recovery; this flow is for live workspaces with history.

## 4. Keep machines in sync

Once the histories are shared, the ongoing cycle is plain
fetch-inspect-pull:

```sh
memstead fetch knowledge     # see what moved (updated refs print per ref)
memstead pull knowledge      # fast-forward + reload, refuses on divergence
memstead push knowledge      # send your own commits
memstead push --all          # every mem branch plus __MEMSTEAD, fast-forward only
```

`push --all` is the whole-workspace form: it lists the remote once, skips
every ref already in sync (a run with nothing to send prints nothing),
prints one line per ref it moved, and names a ref it could not
fast-forward (`NON_FAST_FORWARD`) while still pushing the others, exiting
non-zero at the end. It also carries the `__MEMSTEAD` ref, where schemas
and mem configs live, which the single-mem form has no route for. There
is no `--force` on `--all`.

`pull` validates the incoming tree against the mem's pinned schema
before moving anything (`SCHEMA_VIOLATION_IN_FETCH` refuses the whole
pull), fast-forwards the branch, and reloads the in-memory store — the
graph reflects the pulled state immediately.

Divergence (both sides committed) never merges silently: `pull` refuses
with `LOCAL_DIVERGENCE`, and you choose — `branch-reset` to adopt the
remote, or push from the other machine first.

## 5. Fork a mem at a recorded ancestor

A proposal against a mem is a second lineage, not an unrelated mem: fork
it, and the diff between the two later has a common ancestor to stand on.

```sh
memstead mem fork knowledge knowledge-proposal            # at the source's tip
memstead mem fork knowledge@<sha> knowledge-proposal      # at a commit the source reaches
memstead mem fork knowledge knowledge-proposal --remote origin   # source fetched from the remote
```

The fork's branch starts at the source's commit, its config is the
source's (same schema pin, same version and description) with
`forkedFrom` (source mem, sha, remote when one was used) written in, and
its entities read identical to the source's at that commit. A local fork
carries the source's outgoing cross-link grants under its own name, so
copied cross-mem edges read conformant from the first read; `mem list`
shows the origin (`forked from ...`). The name obeys the same create rules
as `mem init`, and every refusal (a sha not on the source branch, a name
a branch sits above or below, an existing name, a folder or archive
source) lands nothing.

With `--remote`, the source branch and the remote's `__MEMSTEAD` config
are fetched into remote-tracking refs (the local `__MEMSTEAD` never
moves), the fetched tree is validated as `pull` validates before the
mount exists, no grant is inherited (the remote's policy is not this
workspace's), and a schema pin this workspace cannot resolve refuses
naming the remedy, `memstead schema install`.
