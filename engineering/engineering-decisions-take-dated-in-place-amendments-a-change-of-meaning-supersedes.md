---
type: decision
created_date: 2026-09-02T20:10:20Z
last_modified: 2026-09-02T20:10:20Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: schema, engineering, amendments
---

# Engineering decisions take dated in-place amendments; a change of meaning supersedes

## Decision
In the engineering schema (0.4.0 and later) a change that keeps a decision's meaning (a corrected count, a moved path, a later confirmation, a reversed refinement that leaves the core decision standing) is recorded in place as a dated amendment sentence or paragraph, `Corrected <date>:` or `<date> amendment:`; a change of meaning is never edited into an accepted decision, it gets a new decision with SUPERSEDES and the old status flips to superseded.

## Context
Bundle B plan 3 (2026-09-02). Amendments landed as untraceable in-place edits or as duplicate decisions; the schema's write rules said neither. engineering@0.4.0 ships the two rules on the decision type's system message and write rules, and the engineering mem runs on it.

## Consequences
A reader of a decision sees its history inside it, dated, and the SUPERSEDES chain carries meaning changes; the bundle close of the same day used the rule to amend the anchors-merge decision in place.

## Options

Always superseding rejected: it turns every corrected figure into a new entity and loses the reader. Never amending rejected: it leaves the decision lying about a count it once got wrong.
