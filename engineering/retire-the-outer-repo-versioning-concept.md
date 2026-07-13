---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:43:07Z
status: accepted
decided_on: 2026-07-11
deciders: operator
scope: subsystem
tags: plugin, skills, hooks, outer-vcs
---

# Retire the outer-repo versioning concept

## Decision
The outer-repo versioning concept is retired entirely: the auto-commit Stop-hook family (hook, shared pipeline, Memstead-cursor trailers) and the /commit recovery skill are deleted from the plugin. The public skill roster is **seven** in two families (setup, interview, learn / ingest, sync, verify, tidy). The user's project repo is exclusively the user's business — the plugin never commits to it.

## Context
Operator decision, 2026-07-11, after the projection-pipeline bundle completed. The concept's original consumer chain had died out from under it: the Memstead-cursor trailer mechanism existed for the retired /reconcile cursor bookkeeping, the new maintenance loop (sync_state, advance store, findings) is entirely engine-owned, and on the git-branch backend the engine commits every mutation to the mem's own history. For folder mems the operator judged auto-committing into the user's repo overreaching product behaviour — plain markdown files in the user's tree are versioned by the user, on the user's terms.

## Consequences
- The roster drops to seven; plugin and marketplace bumped to 0.4.0; the skills reference regenerates to seven.
- Folder-mem entity files carry no automatic git history — accepted deliberately; the engine still records every mutation in its provenance channels (changes.jsonl on folder mounts), and git-backed mems keep engine-committed history.
- The engine's `outer_vcs` config field is now consumerless — dormant, removal backlogged as an engine-side residue item.
- Engine tool/CLI prose no longer advertises the retired consumer (include_notes descriptions reworded to "commit-mirroring clients").
- Supersedes the roster-8 shape of the projection-pipeline design (operator decision 1 of that bundle) — a deliberate re-decision by the same operator, not drift.

## Relationships
- **REFERENCES**: [[plugin:commit-skill]]
- **REFERENCES**: [[plugin:auto-commit-hook]]

## Options



## Notes

Realized in public plugin release 0.4.0 (skill + hook family deletion, hooks.json, lint roster, docs regen). The retired surfaces' graph records: [[plugin--commit-skill]], [[plugin--auto-commit-hook]] (both marked HISTORICAL, stability frozen).


The "seven skills" roster count recorded in this decision was accurate at decision time; the same-day plugin diet (plugin 0.5.0) later folded /verify into /sync `--verify`, making the roster **six**.
