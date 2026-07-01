---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Mount

## Definition

A mount is the act of making one [[vault]] — together with its [[schema]] and capabilities — available to a running engine, and also the resulting record in the engine's mount registry.

## Explanation

One mount is one vault. A mount record carries the vault being made available, its [[storage-backend]] (where the bytes live), its capability (read or write), its lifecycle (eager or lazy), and whether other mounts may form cross-vault [[wikilink]] edges into it. A [[workspace]] is, at boot, a set of mounts plus the policy over them. Capability is per-mount: the same folder-backed vault can be mounted writable in one workspace and read-only in another.

## Boundaries

- A mount is not a [[storage-backend]]: the backend holds the bytes; the mount is the act and record of attaching a vault with a capability.
- A mount is not a [[vault]]: many workspaces may each mount the same vault differently.

## Significance

Because capability lives on the mount, a single engine can hold a read-only reference [[vault]] beside a writable working vault — the basis for the engine refusing writes to a sealed source.
