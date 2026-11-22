---
type: decision
created_date: 2026-11-22T13:15:47Z
last_modified: 2026-11-22T13:15:47Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: change-notice,provenance,mcp
---

# The detailed tier of a change notice names the newest writer and its note

## Decision
We chose to have each entry in the detailed tier of a change notice carry the declared identity of the newest write to that entity inside the notice's range, and that write's note, each absent when undeclared and never taken from an older write. The source is the record of writes each storage kind already keeps: commit notes on a git-branch mem, the change ledger on a folder mem. A commit message's trailer block is its last paragraph and a note is never parsed for trailers; a declared identity is one line in every record; a batch records `<id>: <note>` per entry with the further lines of a note indented.

## Context
A caller told that entities changed could not tell who changed them or why without a second read of history. Grading the feature also showed defects of the released engine that the notice made visible: a note line shaped like a trailer ended the note or forged the identity, role or actor of the write, and a batch gave every entity the joined note of all entries.

## Consequences
- A reader of a notice sees writer and reason per entity with no further call.
- The larger tiers and the change feed's entries do not carry the two keys.
- No second walk over history is made for it.
- A commit an older engine wrote with a blank line inside its identity reads worse than before.

## Relationships
- **DERIVED_FROM**: [[a-change-notice-is-relative-to-a-caller-key]]

## Options

- A second walk over history per notice: rejected, the write records already hold both facts.
- The two keys on every tier: rejected, the larger tiers exist to stay small.
- The detailed tier, from the existing write records: chosen.
