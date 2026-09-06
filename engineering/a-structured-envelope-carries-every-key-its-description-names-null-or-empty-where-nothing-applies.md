---
type: decision
created_date: 2026-09-06T00:35:32Z
last_modified: 2026-09-06T00:35:32Z
status: accepted
decided_on: 2026-09-06
deciders: reader-walks bundle session 2026-09-06, under the descriptions-and-help-texts plan
scope: subsystem
tags: wire-shape, search, mcp, cli-json, descriptions
---

# A structured envelope carries every key its description names, null or empty where nothing applies

## Decision
We will serialise every key a tool description names as part of a structured envelope on every response, with `null` or `[]` where nothing applies, and we will not elide a key because its value is empty. The search envelope is the first case: `warnings` is `[]` when nothing warned, and a hit's `score_breakdown`, `matched_terms` and `expansion` are `null` on the paths that do not compute them (a metadata-only call, a primary hit), on `memstead_search`, `memstead search --json` and `memstead list --json` alike. A description that names a key promises it; where a value can be `null`, the description says so in the sentence that names the key. A consumer reads a key and branches on its value, never on its presence.

## Context
The reader walks of 2026-09-04 found the `memstead_search` description listing envelope and hit fields the wire omitted: `warnings`, `score_breakdown`, `matched_terms` and `expansion` were serde-skipped when empty or absent, so an agent parsing the promised shape met missing keys on the common path. Two repairs were possible, documenting the omission or serialising the keys; the plan named the second as its default because a stable envelope is what the description already promised, and the session found no consumer that depended on the omission (the wire-shape tests pinned presence only).

## Consequences
- Every search and list response carries the same top-level and per-hit keys; a consumer's parser has one shape to handle.
- A roster of many hits carries three `null` fields per hit it did not carry before; the bytes are the accepted cost of the stable shape.
- Adding a new envelope key follows the same rule: always present, described with its null case, never conditional on emptiness.
- Fields a description does not name (`last_modified`, `sections` on a hit) keep their serde omission; the promise binds what is named.

## Options

- Document the omission in the description ("appears only when non-empty"): rejected, it makes the description true by weakening the promise and leaves every consumer testing key presence.
- Serialise every named key on every response: chosen.
- Serialise every field of the hit struct unconditionally: rejected, `sections` and `last_modified` are unnamed by the description and their omission costs no consumer a branch.
