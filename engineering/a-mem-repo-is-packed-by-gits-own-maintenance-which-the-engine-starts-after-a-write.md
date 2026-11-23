---
type: decision
created_date: 2026-11-23T11:12:59Z
last_modified: 2026-11-23T11:23:33Z
status: accepted
decided_on: 2026-11-23
deciders: operator (backlog bundle, go of 2026-11-23), implementing agent
scope: subsystem
tags: git-branch,mem-repo,storage,git,maintenance
---

# A mem-repo is packed by git's own maintenance, which the engine starts after a write

## Decision
We chose to have the engine run `git gc --auto` for the git directory it has just committed into, once git's own estimate says enough loose objects have gathered there (more than 27 loose objects whose hash begins with `17`, about 6,700 in all at git's default). git's configuration of that directory decides whether and how: `gc.auto = 0` switches the pack off, `gc.autoDetach` decides whether git packs in the background, `gc.pruneExpire` how long an unreferenced object is kept. The engine passes no setting and no prune expiry of its own, starts no process below the threshold, asks at most every few seconds per git directory from one process, waits for the process it started for a bounded time (thirty seconds, then the pack runs on), and never fails a write over the pack: the commit has landed by then. A user may pack by hand with `git gc` at the default prune grace beside a running engine; the backup guide says so.

## Context
Every write to a git-branch mem stores loose objects through the git library, three for a create, and nothing packed them: git packs a repository from its own commands, and the engine never runs those on a write. A store grew by loose objects without bound. No page said who packs, and the rule that nothing outside the engine mutates a mem-repo read as if a user's `git gc` were forbidden.

## Consequences
- It needs `git` on the search path; without it nothing is packed and no write says so.
- The engine runs the `git` binary for a local step now, beside the network transport and the working-tree sync.
- Whether the write waits for the pack is git's setting too (`gc.autoDetach`, detached by default), and the engine leaves it alone: git's guard against packing again and again, when young unreferenced objects alone exceed the threshold, is the log file its detached mode writes.
- Whether a pack is under way, or was tried and is not to be tried again yet, is git's to judge by its own lock and log file; a first build judged those itself, and a stale log switched the pack off for good.
- A pack with an immediate prune expiry is not covered and can destroy a mem beside a running engine: a write's objects are protected by the prune grace until its branch points at them.
- A pack and its `gc` settings are the one exception to the rule that nothing but the engine touches a mem-repo's git directory.
- Folder mems have no object store and are not touched.

## Relationships
- **DERIVED_FROM**: [[use-subprocess-git-for-network-transport-instead-of-gix-http-transports]]

## Options

- A `memstead` verb that packs: rejected, the surface freeze stands and git already has the verb.
- Document `git gc` and pack nothing: rejected as the whole answer, a user who never reads that page keeps a store that grows.
- Pack inside the engine with the git library: rejected, a second implementation of git's maintenance.
- Pack on a schedule or at boot: rejected, a one-shot CLI process boots thousands of times, and a schedule needs a process that stays.
- An engine setting to switch the pack off: rejected, git's `gc.auto` in that directory says it.
- Leave the refs unpacked by passing `gc.packRefs=false`: tried and rejected, the maintenance git runs after a fetch packs the refs whatever the engine passes, so the cause had to go instead.
- `git gc --auto` after a write, git's configuration deciding: chosen.
