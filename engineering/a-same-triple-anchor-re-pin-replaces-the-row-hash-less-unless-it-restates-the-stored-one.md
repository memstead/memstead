---
type: decision
created_date: 2026-09-02T20:10:22Z
last_modified: 2026-09-02T20:10:23Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: anchors, provenance, mutation-surface
---

# A same-triple anchor re-pin replaces the row hash-less unless it restates the stored one

## Decision
Writing an anchor that names a stored (artifact, grain, class) triple has three shapes on every mount kind: a row that restates the stored one on every caller-supplied field is a no-op that writes nothing, commits nothing and answers `anchors_changed: false` with `UPDATE_NOOP`; a row that differs in any supplied field replaces the stored row hash-less for the next verify to backfill; an update that also changed the entity's content re-baselines even a restated row. A supplied hash sets the baseline directly; `anchors_unset` stays the explicit removal. Every update response that carried anchors says `anchors_changed`.

## Context
Bundle B plan 10 (2026-09-02). The 2026-08 consistency sweep's refinement carried the stored baseline forward on a same-triple re-pin so drift stayed falsifiable, which made the sync brief's one-update repair (content plus the same anchors) keep the old baseline and read `drifted` against the content it had just repaired; the anchors-only restatement produced an empty commit on git-branch mems and silence on folder mems.

## Consequences
This reverses the sweep's carry-forward refinement recorded in the anchors-merge decision, amended in place the same day. Nothing the sweep protected is lost: a re-pin is itself recorded (an `anchor` commit, a ledger line), and a restated row moves no baseline.

## Relationships
- **SUPERSEDES**: [[anchors-merge-with-explicit-unset-instead-of-silently-replacing]]

## Options

Keeping the carry-forward and documenting a two-step unset-then-write recipe rejected (it leaves the update contract false and moves the workaround into every agent's head); refusing anchors-only updates rejected (the brief instructs them).
