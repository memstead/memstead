---
type: concept
created_date: 2026-07-15T07:24:16Z
last_modified: 2026-07-15T07:24:16Z
maturity: established
abstraction_level: abstract
tags: effects, core-concept
---

# Effect Verb

## Definition
An effect is a declared capability a Kāra function may exercise on some resource — drawn from six built-in verbs (`reads`, `writes`, `sends`, `receives`, `allocates`, `panics`) plus user-defined resources — that the compiler tracks, infers, and verifies across every call site.

## Explanation
Every function has an effect signature. The built-in verbs describe interactions with state and I/O; `allocates` marks heap use and `panics` marks abnormal exit. Users declare their own resources and parameterize effects over them (e.g. `writes(Db)`). The effect checker infers effects bottom-up, verifies declared-vs-actual with subtyping at call sites, and unifies `with E` handler regions. Because effects encode read/write dependencies, they are also the input to auto-concurrency: two regions with disjoint effects can run in parallel.

## Relationships
- **REFERENCES**: [[colored-functions]]
- **REFERENCES**: [[tiered-ownership-model]]
- **REFERENCES**: [[auto-concurrency]]
- **REFERENCES**: [[effect-checker]]

## Boundaries

- Not exceptions: `panics` is one verb among six, not the whole system.
- Not async coloring: an effect signature does not split functions into sync/async worlds — it is data the scheduler reads, the opposite of [[colored-functions]].
- Not ownership: effects track what a function does to resources; [[tiered-ownership-model]] tracks who owns the memory.

## Significance

Effects are the keystone of Kāra's design — they drive [[auto-concurrency]], appear in the type surface, and are enforced by the [[effect-checker]]. Understanding them is prerequisite to understanding why the language needs no async/await.
