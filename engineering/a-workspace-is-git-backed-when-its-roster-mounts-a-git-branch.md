---
type: decision
created_date: 2026-11-23T13:02:32Z
last_modified: 2026-11-23T20:28:33Z
status: accepted
decided_on: 2026-11-23
deciders: operator (backlog bundle, go of 2026-11-23), implementing agent
scope: system
tags: workspace-shape,mounts,views,cli,policy
---

# A workspace is git-backed when its roster mounts a git branch

## Decision
We chose to have the one probe that tells a git-backed workspace from the shape made for one folder mem read the mount roster as well as the directory: a workspace is git-backed when it holds a `mem-repo/` git directory or when its roster mounts a git branch, wherever that branch's git directory lies. A workspace made with `memstead mem mount --new-workspace` keeps engine state only and mounts mems out of another workspace's git directory; it is a full workspace with several mems, takes the policy commands, the dump and the batch verbs, and a create there may name any of its mounts. Its policy file is its own: the workspace it mounts from is not written.

## Context
The probe looked for a `mem-repo/` directory only. A workspace that mounts git branches of another workspace's git directory has none, so it read as the folder shape, which serves exactly one mem: `workspace grant-cross-link`, `workspace dump` and `batch-create` refused with `UNSUPPORTED_WORKSPACE_SHAPE`, and a create on the command line that named any mem but the first on the roster, the writable ones included, was answered `UNKNOWN_MEM` (the MCP server already created there). A server that gives each caller a workspace of its own over one shared store could not grant a cross-mem link there, and a write into a read-only mount was answered as an unknown name.

## Consequences
- A write into a read-only mount of such a workspace answers `READ_ONLY_MOUNT` wherever the mount stands on the roster; a name it does not mount answers `UNKNOWN_MEM`.
- A workspace that holds only folder mems is still the folder shape and still refuses the commands that need a git-backed workspace.
- The probe reads `.memstead/state/mounts.json` once more per call that asks for the shape; a roster that cannot be read counts as no git mount.
- The two shapes stay two: the decision widens which workspaces count as git-backed and removes neither shape.
- A view hosts no mem of its own: `mem init` refuses there with `UNSUPPORTED_WORKSPACE_SHAPE` as it did before a view counted as git-backed, and the engine's mem create refuses behind it (`INVALID_INPUT`, naming the way: create the mem where the git directory is and mount it). A folder mem in a view would leave the workspace in the folder shape around it once the git mounts are taken off, with no verb left that removes it.
- A view whose last mount was taken off reads as the folder shape until a mount returns; its refusals say that the workspace mounts no mem and name `memstead mem mount`. Counting an empty roster as git-backed was tried and withdrawn the same day: it let `mem init` create that folder mem.
- Two questions, two probes (2026-11-23, operator decision after an independent check). Whether a workspace is git-backed for what its mounts hold is this decision's probe. Whether it is the store those branches live in is a second one, `has_own_mem_repo`: the workspace carries `mem-repo/.git/`. The verbs that act on the store itself ask the second and refuse without it as the released binary did: `mem init`, `mem delete`, `mem rename`, `branch-reset`, `fetch`, `pull`, `push`, `mem-repo remote-add`, `recover`. The first build asked one probe for both and so opened them: in a view, `mem delete` and `mem rename` removed and moved a branch in the other workspace's git directory, and in a folder workspace that also mounts a branch, `mem delete` of its own mem removed the workspace's directory.
- The list of what opens is closed and was taken by script: every leaf command of the CLI and the lifecycle tools of the MCP server, run in a view and in a folder workspace with a branch mount on the released and on the new binary. What differs is `workspace dump`, `workspace grant-cross-link`, the batch verbs and a create that names a mount. A new command that takes its engine by the widened probe joins that list by decision, not by default: a verb that acts on the store takes `store_engine`.
- The MCP tool `memstead_mem_delete` asks the second probe too (2026-11-23): without an own `mem-repo/` it refuses a mount of a git branch, as the CLI's verb does. It had no gate, and from such a workspace it deleted the branch in the other workspace's git directory, on the released binary too.
- A mem delete removes only a folder that is the mem's to remove (2026-11-23, engine rule for every surface): not a folder that holds this workspace, and not one that, walking up from it, meets a workspace root (this workspace's or another's) and is that root itself or lies in its `mem-repo/` or its `.memstead/`. A create starts no folder mem there; the root of a workspace without a `mem-repo/` is where a folder workspace keeps its one mem and stays. Before, a delete of a folder workspace's one mem removed the workspace's directory with every other file in it, and a folder mem named `mem-repo` took every branch along. The rule reads the path upward only: a folder that lies above another workspace without being one, and a folder mem whose folder holds another mem's folder, are not seen (decided limit; an independent check kept finding members of that class, each needing operator mode or a hand-made roster to create).
- Undecided and outside this decision: what a caller of a view may do to the store beyond its mounts' content (rights per caller), and that a workspace with its own `mem-repo/` that mounts another workspace's branch still deletes that branch there (backlog).
  2026-11-23 amendment: the first half was decided the same day. Callers with different rights are served through one view per set of rights, a view takes from the store only the schemas its mounts pin, and a pin is written only where the schema is installed; rights per caller inside one engine were not built. See [[engineering--serve-callers-with-different-rights-through-one-view-per-set-of-rights-a-view-takes-the-schemas-its-mounts-pin]]. The second half stays open in the backlog.

## Relationships
- **INFORMED_BY**: [[every-workspace-creating-command-discloses-the-shape-it-just-made]]
- **REFERENCES**: [[serve-callers-with-different-rights-through-one-view-per-set-of-rights-a-view-takes-the-schemas-its-mounts-pin]]

## Options

- Give the policy commands their own probe: rejected, the same wrong answer stood behind the batch verbs and behind create.
- Make `--new-workspace` lay an empty `mem-repo/`: rejected, an empty git directory that holds no mem is a second place a mem could be created by mistake.
- Record the shape in the workspace file at creation: rejected, existing workspaces would carry no record and the roster already says it.
- One probe that reads the roster too: chosen.
