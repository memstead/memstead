---
title: Give each caller a view of a store
description: "Serve several callers from one mem-repo so that each reaches only the mems released for it, read-only or writable, with the verbs that exist, and know what a view does not hold."
sidebar:
  order: 10
---

A program that serves several callers from one mem-repo often wants
each of them to reach only some of its mems: this one may read `facts`
and change `notes`, that one may read `notes` and nothing else. The
engine has one form for this. A **view** is a workspace of engine state
only, with no mem-repo of its own, whose roster mounts branches of the
store's git directory, each with a capability. One view per set of
rights, one engine (or one `memstead-mcp` process) per view.

This guide makes a view, says what each capability carries, and says
what a view is not.

## 1. Make a view

The store is an ordinary mem-repo workspace (`memstead mem-repo init`).
A view is made in an empty directory of its own. The first mount lays
the workspace, every further one adds a row to its roster:

```sh
mkdir /srv/views/team-a
memstead --workspace /srv/store mem list         # the mems the store holds

memstead mem mount notes --branch notes \
  --gitdir /srv/store/mem-repo/.git --capability write \
  --new-workspace --operator-mode                # run in /srv/views/team-a
memstead mem mount facts --branch facts \
  --gitdir /srv/store/mem-repo/.git --capability read-only \
  --operator-mode
```

A view starts with no create rule, so a mount needs `--operator-mode`:
mounting is the act of whoever sets the view up, not of the callers it
will serve. Nothing is written into the store by a mount.

Serve the view like any workspace: `memstead-mcp --workspace
/srv/views/team-a`, without `--operator-mode`, or an engine booted from
that directory by a program that links the engine.

To change a view later, change its roster: `memstead mem unregister
facts --operator-mode` withdraws a mem (the store keeps the branch),
and a mount with another `--capability` after an unregister changes
the right. A running engine follows its roster from its next operation
on; no restart is needed.

## 2. What each capability carries

| For a mem that is | a caller of the view's MCP server |
|---|---|
| not mounted | reaches nothing of it. A call that names it answers exactly as a call that names a mem that does not exist; its entities, its name and a schema that only it pins appear in no reply (one exception, an id that a mounted mem links to: see the limits). |
| mounted `read-only` | reads its entities, its schema and its history. Every write refuses `READ_ONLY_MOUNT`, and its branch does not move. |
| mounted `write` | also creates, updates, relates, renames, retypes and deletes its entities, records checks on them, and changes the mem's settings (below). |

No capability lets a view create, delete or rename a mem or move a
branch: those acts belong to the workspace that owns the git
directory, and a view refuses them. A schema installed from a view
stays in the view's own folder.

Links between mems follow the view's own policy
(`memstead workspace grant-cross-link`), which starts empty: a writable
mem links into another mounted mem only where the view grants it. A
running server takes a changed policy at its next `memstead_reload`
without a mem, or at its next start.

## 3. Withhold the mem's settings

`write` includes three tools that change the mem for every workspace
that mounts it: `memstead_mem_set_version`, `memstead_mem_configure`
(title, description, subject) and `memstead_mem_set_schema`. A host
that wants its callers to change entities and nothing about the mem
switches those tools off for the view, in the view's
`.memstead/workspace.toml`:

```toml
[mcp]
disabled_tools = [
  "memstead_mem_set_version",
  "memstead_mem_configure",
  "memstead_mem_set_schema",
]
```

A disabled tool is absent from the tool list and a call to it refuses
`TOOL_DISABLED`. A program that links the engine passes the same set
to the server it builds.

Where the pin switch stays on, it moves a mem only to a schema the
store can resolve: a built-in one, or one installed in the store. A
schema installed in the view alone refuses `SCHEMA_NOT_FOUND`, because
a pin is written where the mem lives and the store would lose the mem.
From a view a mem moves to a built-in schema, to one that another mem
mounted from the same store pins, or to one the view has installed
itself and the store holds too. Any other move is made in the store.

When the store moves a mounted mem to another schema, a view follows
the pin at its next operation. An engine already running in the view
validates the next write against the new schema, reads the mem under
it and names the new pin in `_mem_schema`; no restart and no reload is
needed. The view's own roster keeps the pin the mem was mounted under,
and the view's health read reports the difference as
`SCHEMA_PIN_MISMATCH`. A migration the store has started and not
completed changes nothing in a view: the store's pin moves when the
switch completes. If the store moves a mem to a schema the view cannot
resolve (one the store holds only in its own `.memstead/schemas/`
folder), the mem goes out of service in the view, `MEM_QUARANTINED`
with the reason `SCHEMA_NOT_FOUND`, until the schema is installed in
the store's mem-repo and the view is reloaded with `memstead_reload`.

## 4. What a view is not

A view is a configuration, not a wall. It decides what the engine
serves through it. It does not stop someone who can reach the files:

- Whoever can write the view's directory can add a row to its roster,
  and whoever can run `memstead` there with `--operator-mode` can mount
  any branch of a git directory they can read.
- Whoever can read the store's git directory can read every mem in it
  with git.

Keeping callers off the files and off the command line is the host's
part: give them the MCP surface of their view and nothing else.

There are no rights per caller inside one engine and none narrower
than a mem. Callers that share a view share its mounts. The engine
verifies no one's identity; a view says which mems are reached, not
who reaches them.

## 5. Limits to know

- **A linked id is visible.** If a mounted mem links to an entity of a
  mem the view does not mount, the view shows that id as a stub: the
  id (which carries the mem's name), a title derived from the id, and
  nothing of the entity's content or type.
- **Integrity checks see the view's mems only.** A delete or rename
  from a view does not know of entities in unmounted mems that link to
  the target. The reference is left dangling there, and the store's
  health read shows it: `memstead health --include dangling_links,stubs`
  lists a body link under dangling links and a relation's target under
  stubs.
- **Checks are recorded per workspace.** A check recorded through a
  view lands in that view's own ledger; another view, and the store,
  do not see it.
- **A check needs a writable mount.** `memstead_check` on an entity of
  a read-only mount refuses `READ_ONLY_MOUNT`.
- **A running engine keeps a schema it has resolved.** After a mem is
  withdrawn from a view, the schema it pinned stays readable through
  that view until its engine restarts.
