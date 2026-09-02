---
type: decision
created_date: 2026-09-02T20:10:22Z
last_modified: 2026-09-02T20:10:22Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: projection, exclusions, verify
---

# projection exclude resolves the artifact id at write time and refuses an unknown one

## Decision
An exclusion id is resolved at exclude time through the binding's source join, the way the anchor write gate resolves an artifact path: an id already among the enumerable source artifacts is taken as is, otherwise the source-relative form is joined onto each primary source's medium base and made workspace-relative, and the ledger holds that one canonical id. An id resolving to no artifact refuses the whole call with the existing typed code, naming the nearest known ids in the message and the payload, recording nothing; the response lists each requested id beside the canonical one.

## Context
Bundle B plan 11 (2026-09-02). The flagship sync found only the joined form took effect on the findings filter; the engine already refused the source-relative form as a non-member, but without resolving it or naming what would have matched.

## Consequences
Exclusion files written before keep working (one canonical spelling); an agent sees the spelling that took effect and, on a typo, the ids it meant.

## Options

Accepting both forms and matching either at filter time rejected (two spellings in every reader); warning instead of refusing rejected (a warning beside a stored no-op is the silent failure the entry was filed on).
