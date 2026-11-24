---
type: decision
created_date: 2026-11-23T20:14:54Z
last_modified: 2026-11-24T09:56:23Z
status: accepted
decided_on: 2026-11-23
deciders: operator (requirement and go of 2026-11-23), implementing agent
scope: system
tags: views,mounts,schemas,rights,mcp
---

# Serve callers with different rights through one view per set of rights; a view takes the schemas its mounts pin

## Decision
We chose one form for serving several callers from one mem-repo with different rights, and made that form hold what it says: a view, a workspace of engine state only whose roster mounts branches of the store's git directory, each with a capability. A mem that is not mounted is not served, a read-only mount is read, a writable mount is read and written. Two rules were added to the engine for it. A workspace takes from another git directory only the schemas that the mems it mounts from there pin (the pin in the mem's configuration there, the pin and the migration target on its roster row); its own mem-repo it reads whole. And a schema pin is written only where the schema is installed: a pin switch on a mem of another git directory refuses a target that is neither built in nor installed there. Rights per caller inside one engine, a verified identity in the record, and an engine gate for a mem's settings were not built.

## Context
A program that serves several callers from one store of mems wants each caller to reach only the mems released for it, each for reading or for changing. The form existed since 0.27.0 (see [[engineering--a-mem-that-exists-as-a-branch-is-mounted-by-a-cli-verb-that-writes-a-roster-row]] and [[engineering--a-workspace-is-git-backed-when-its-roster-mounts-a-git-branch]]). It was measured before anything was built, on the released 0.27.1 and on the tree: a store of four mems, a view that mounts two, every MCP tool, each call naming an unmounted mem paired with the same call naming a mem that exists nowhere, and text only the unmounted mems hold searched in every reply and every file of the view. Nothing of an unmounted mem's content came out and a read-only mount took no write. Two things did not hold. The view resolved every schema installed in the store, also one that only an unmounted mem pins: since 0.26.1 a workspace reads the schema registry of every git directory it mounts from, and it read all of it. And a view could pin a mounted mem to a schema installed in the view alone; the pin is written into the mem's configuration, which lives in the store, and the store then quarantined the mem for every workspace.

## Consequences
- The set of schemas a workspace holds is built without the schemas of another git directory that no mounted mem pins. There is one reader of a registry, and boot, the warm refresh, the below-boot repair and the CLI's schema read come through it. No reader of the set filters for itself.
- From a workspace that does not own a mem's git directory, the mem is moved only to a built-in schema, to one that another mounted mem of that directory pins, or to one the workspace has installed itself and the store holds too. A move to any other schema of the store is made in the store. This is a behaviour change for whoever switched pins from a view; the changelog states it.
- The refusal of a pin the mem's git directory lacks shares `SCHEMA_NOT_FOUND` and names the git directory in its details; no new error code.
- A mem's settings (version, title, description, subject, a pin to a built-in schema) stay part of what a writable mount carries. A host that withholds them switches the three tools off per workspace with `[mcp].disabled_tools`. The cost: a host that forgets the line lets its callers change a mem's settings for everyone.
- A view is a configuration, not a wall. Whoever can write the view's directory or run the CLI there in operator mode can mount any branch of a git directory they can read. Keeping callers off the files is the host's part, and the guide on the documentation site says so.
- The suite holds the contract as one test over every tool the MCP server lists; a tool added later fails the test until it has a call there.
- One thing of an unmounted mem is visible by design: where a mounted mem links to one of its entities, the view shows that id as a stub. The id carries the mem's name; no content, type or title of the entity is served. The id is content of the mounted mem. The suite's test and the guide except it by name.
- Left open and stated as limits: an engine already running keeps the schema pin it booted with after the owning workspace moved it, and keeps a schema it has resolved after its mem is withdrawn; integrity checks of a delete or rename see only the mems the view mounts; checks are recorded per workspace; a check on a read-only mount refuses.
  2026-11-24 amendment: the first of these limits is retired. An engine already running now follows a pin the owning workspace moved at its next operation, and every reply names the mem's pin from its configuration; see [[engineering--a-running-engine-follows-a-schema-pin-moved-where-the-mem-lives-and-every-reply-names-the-configurations-pin]]. The other limits stand.

## Relationships
- **REFERENCES**: [[a-mem-that-exists-as-a-branch-is-mounted-by-a-cli-verb-that-writes-a-roster-row]]
- **REFERENCES**: [[a-workspace-is-git-backed-when-its-roster-mounts-a-git-branch]]
- **REFERENCES**: [[a-running-engine-follows-a-schema-pin-moved-where-the-mem-lives-and-every-reply-names-the-configurations-pin]]

## Options

- One view per set of rights, with the engine closing the gaps a test proves: chosen.
- Rights per caller inside one engine, as a filter on what each call may see: rejected. One engine holds one store across its mounts and computes its derived data (communities, incoming edges, stubs, suggestions) over all of them; a filter at every exit is a class of gates without a last member, and 0.27.1 had already shipped a fix for one such exit.
- A verified identity recorded beside the caller-declared one: rejected for this need. The requirement is which mems a caller reaches, not who the caller is proven to be.
- Hiding the foreign schemas at each reader of the schema set: rejected. At least eight readers, and each later one would have to remember the rule.
- Refusing every pin switch and every setting from a workspace that does not own the git directory: rejected. Broader than the defect; a switch to a built-in schema harms no one, and the existing key withholds the tools where a host wants that.
- A new verb that makes a view from a list of mems and rights: rejected under the surface freeze. Two commands per view are a recipe a host generates.
- Stopping to read another git directory's schemas at all and asking for an install in the view: rejected. It reverses the fix of 0.26.1, which made a mounted mem resolve the schema its own git directory holds.
- A policy key or a capability that decides which schemas of another git directory a workspace may read: rejected. There is nothing to configure; a schema that no mounted mem pins has no reader in that workspace.
- Installing the schema into the mem's git directory as part of a pin switch from a view: rejected. A view would write the store's schema registry, and the registry of a mem-repo is written by the workspace that owns it; a schema installed from a view stays in the view's own folder.

## Notes

2026-11-24 amendment: the operator confirmed the form after the release. Asked whether the view form suffices for what he had called the rights system on 2026-11-23, or whether a verified identity in the record is still wanted, with the recommendation that the view form suffices and that verified identities are taken up only when a concrete case asks for them (two parties that do not trust each other and could dispute who wrote something), as work of its own, he answered: "dann alles das jetzt so machen wie du es empfohlen hast". Released in 0.28.0. Revisit condition: such a case, or a host that reports a measured failure of the view form.
