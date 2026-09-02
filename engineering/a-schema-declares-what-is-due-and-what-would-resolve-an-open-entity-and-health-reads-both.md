---
type: decision
created_date: 2026-09-02T20:10:21Z
last_modified: 2026-09-02T20:10:21Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: schema, health, due, resolution
---

# A schema declares what is due and what would resolve an open entity, and health reads both

## Decision
A type may declare a `due` axis (a date field, a status field with its open values, a lead section) and a `resolution` axis (a condition section, an optional status field with open values, a check kind). The due brief reads overdue and due-soon rows with the days past or until, and the `open_questions` health axis reads `resolution_missing` (open entities whose condition section is empty) and `resolution_unchecked` (open entities whose condition no ok check of the declared kind covers at the current content), composed from the ledger with no new stored state. `project@0.5.0` declares due on milestones; the workspace-local `planning@0.6.0` declares resolution on open questions and acceptance criteria.

## Context
Bundle B plans 4 and 5 (2026-09-02). The engine could not say what was overdue or what would settle an open question without reading prose; the two declarations are readings, not gates: the write path refuses nothing new and the engine never judges a condition.

## Consequences
A criterion's assertion is its own resolution condition (no status field means open in every entity), so a plan's unchecked criteria enumerate themselves; a milestone past its target date reads overdue with its blockers quoted. The `MEMSTEAD_TODAY` pin and the `--today` flag inject the clock for fixtures.

## Options

A new health axis for due rejected: the due brief is the data surface and the constraints forbid a second clock; a stored resolution state rejected: the check ledger already holds it.
