---
type: decision
created_date: 2026-11-22T13:15:48Z
last_modified: 2026-11-22T13:15:48Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: mem-repo,mount,cli,roster
---

# A mem that exists as a branch is mounted by a CLI verb that writes a roster row

## Decision
We chose one CLI verb, `memstead mem mount`, that mounts a mem which already exists as a branch, of the workspace's own mem-repo, of another mem-repo's git directory, or of a configured remote from which it is fetched first, with a chosen capability. It writes a roster row and lets the roster reconciliation attach it; it lays no storage and no seed commit. The schema pin is read from the registry of the git directory that holds the branch; where that has no entry the operator names it, and for a git directory the workspace does not own it stays on the roster row. A mount never re-pins and never guesses a pin. Everything that can refuse a mount on its inputs is judged before a branch, a registry entry or a roster row is written. Removal is the verb that already unregisters a mem.

## Context
At 0.26.1 no verb wrote a roster row for an existing branch. A workspace that mounted mems of another mem-repo had its roster file written by hand, and a mem pushed alone from another store was adopted through `mem init`, `fetch` and `branch-reset` over a throwaway seed commit. The first cut of the verb pruned a registry entry that stood before a refused call, which then travelled to the remote.

## Consequences
- A workspace of engine state only, with no mem-repo of its own, is a supported shape.
- Adopting a mem pushed alone is one command and keeps its history, so a later pull fast-forwards.
- A refused mount from a remote leaves its fetch behind and nothing else: the fetch comes first because the remote's registry may name the pin.
- The verb is CLI-only; a running engine follows the mount and its removal through the roster.
- A single-mem push still does not publish the mem's registry entry, so a mem pushed alone arrives without its pin.

## Relationships
- **DERIVED_FROM**: [[a-roster-row-is-the-same-mount-only-while-its-capability-storage-and-pin-stand]]
- **DERIVED_FROM**: [[engine-owns-mem-repo-state]]

## Options

- An adopt flag on `mem init`: rejected, init creates storage and a mount creates none.
- `mem init --reattach` for adoption: rejected, it lays a seed commit after which a pull refuses as diverged.
- An MCP tool: rejected, mounting is administration.
- Keep the hand-written roster row: rejected, the roster is engine-managed state with a private format.
- A new unmount verb: rejected, the unregister already is the removal that leaves storage in place.
- One mount verb: chosen.
