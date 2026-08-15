---
type: decision
created_date: 2026-08-15T04:20:01Z
last_modified: 2026-08-15T04:20:01Z
status: accepted
decided_on: 2026-08-15
deciders: operator, implementing agent
scope: subsystem
tags: wasm, archive, self-contained, read-surface, browser
---

# A sealed mem is readable from the archive alone

## Decision
A consumer holding a `.mem` archive and the library that reads it needs nothing else to find out what the archive contains. The wasm surface gains `entityIds(mem?)` — the sorted entity ids in the hydrated snapshot, optionally narrowed to one mem, with an unknown mem answering with an empty array rather than throwing.

Deliberately a read accessor and nothing more: no search index, no traversal, no write path. The enumeration gap is what blocked the package's stated purpose; the remaining gaps are documented honestly in the surface parity matrix and stay there.

See [[engine--wasm-browser-engine-js-api]] and [[engine--mem-archive-export-surface]].

## Context
The published package exposes `fromSnapshot`, `applyCommit`, `getEntity(id)`, `health()`, `memNames()` and a typed-refusal `search()`. `getEntity` needs an id, and nothing on the surface produced one — no list, no id enumeration of any kind. So a browser handed only a `.mem` could render nothing.

The 2026-08-14 cold-start run hit this and worked around it by generating an `ids.json` from `memstead list --json` on the CLI, then observed that doing so defeats the point of shipping a self-contained snapshot. `Store::all_ids()` already existed natively; the omission was on the binding, not in the engine.

The alternative considered was documenting the pairing — a sentence in the README saying a snapshot must travel with an id list, which the run said it would have accepted, and which costs nothing. It was rejected because the package's own README states a purpose the omission defeats: shipping an archive that advertises self-containment and is not self-sufficient is the contradiction the finding is actually about.

## Consequences
A published archive is now a complete unit for a read consumer: hydrate, enumerate, read. The browser demo path no longer needs a CLI in the loop.

The ids are sorted, so a page rendering the list is stable across reloads of the same snapshot — a UI detail that would otherwise be re-derived by every caller.

The accessor's narrowness is load-bearing and should stay: each addition to this surface is a claim the browser build can honour, and search genuinely cannot be. The parity matrix documenting what the surface lacks is more useful than a surface that pretends.

The method ships to npm separately — the publication is its own act, not implied by this decision.

## Relationships
- **REFERENCES**: [[engine:wasm-browser-engine-js-api]]
- **REFERENCES**: [[engine:mem-archive-export-surface]]

## Options



## Notes


