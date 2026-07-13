---
type: decision
created_date: 2026-07-13T16:43:08Z
last_modified: 2026-07-13T16:43:08Z
status: accepted
decided_on: 2026-06-08
deciders: memstead-core
scope: system
tags: concurrency, locking, mutation-path, optimistic, engine
---

# Use optimistic content-hash locking for all mutations

## Decision
We chose optimistic concurrency control as the universal mutation-locking mechanism: every write passes an `expected_hash` token (the content hash the caller last read), and the engine refuses the write with `HASH_MISMATCH` when the on-disk content has advanced past that token. There is no pessimistic lock and no last-write-wins fallback — the same token threads uniformly through create, update, delete, rename, and relate. At the git-branch backend the check is enforced as a commit-tip compare-and-swap, mapped onto the same `HASH_MISMATCH` envelope so an agent sees one code regardless of whether the conflict was detected at the entity-hash or the commit level.

## Context
The engine is built for concurrent access: a long-lived MCP engine instance can share one git-branch mem-repo with out-of-band CLI siblings, and a single agent reads an entity, reasons over it, then writes back in a later turn. Between read and write the underlying content may have advanced. A locking discipline was needed that (a) never silently overwrites a change the agent did not see, (b) does not require a stateful server-side lock that a crashed or slow agent could hold indefinitely, and (c) presents a single uniform contract to agents across every mutation path. This pairs with [[engine--reload-before-operation-coherence]]: the pre-operation reload surfaces sibling drift, and optimistic locking catches any stale-parent write that slips past it.

## Consequences
- A write against stale content fails loudly (`HASH_MISMATCH`, with the current on-disk hash in `details.current`) instead of clobbering unseen changes — the agent re-reads and retries.
- No server-side lock state: nothing to leak, time out, or deadlock; a crashed agent holds nothing.
- The same `expected_hash` contract covers every mutation, so agents learn one concurrency model rather than per-operation rules.
- Cost: agents must thread the hash through every mutation (read-before-write is mandatory) and handle the retry loop on conflict; the engine provides `dry_run` as the designated stale-hash recovery path (returns the current `_hash` plus a `prospective_hash`).

## Relationships
- **REFERENCES**: [[engine:reload-before-operation-coherence]]

## Options

- **Last-write-wins** (rejected): accept every write, letting the latest overwrite whatever was there. Rejected because it silently destroys concurrent changes the agent never observed — unacceptable for a graph whose integrity depends on not losing edges.
- **Pessimistic server-side lock** (rejected): hold a lock from read to write. Rejected because it requires stateful lock management, and a slow or crashed agent could hold a lock indefinitely; it also fits poorly with the stateless git-commit model the storage backend already provides.
- **Optimistic content-hash CAS** (chosen): no lock; detect the conflict at write time via a content-hash / commit-tip compare-and-swap and refuse with a uniform error.

## Notes

The token surfaces as `_hash` on every entity read and is required as `expected_hash` on update, delete, and rename. Enforced piecewise across the mutation modules; the git-branch backend's commit-tip CAS is mapped onto the same envelope in `crates/memstead-base/src/storage.rs` so agents see a single `HASH_MISMATCH` code.
