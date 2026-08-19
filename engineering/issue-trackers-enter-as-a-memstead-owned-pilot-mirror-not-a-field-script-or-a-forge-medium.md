---
type: decision
created_date: 2026-08-19T03:28:35Z
last_modified: 2026-08-19T03:28:35Z
status: accepted
decided_on: 2026-08-19
deciders: operator (WOENENN needs statement 2026-08-18, ratified; bundle decision 27), implemented in backlog-sweep plan 03b
scope: subsystem
tags: ingest, issue-tracker, forge-medium, pilot, mirror
---

# Issue trackers enter as a memstead-owned pilot mirror not a field script or a forge medium

## Decision
GitHub issue archives become a memstead source through a memstead-owned, generic, PILOT-GRADE mirror tool (`public/scripts/mirror-issues.mjs`) — not a field-side script and not (yet) a first-class forge medium. The tool mirrors any repository's issues (body, comments, labels, state, dates) one-file-per-issue into a dedicated git repo it commits to only when the tree changed; re-runs against an unchanged tracker are byte-identical, so git itself is the change signal a normal `filesystem` binding consumes — enumeration, change detection, source-dialect anchors, sync, and prune, with no engine change beyond the anchor-dialect work. The tool is explicitly not a stable `memstead` CLI surface: promoting issues to a forge medium is decided from the evidence mirrors like this produce.

## Context
The first external field program needs its ~1,350 GitHub issues as a source within weeks — the issue tracker is the densest "why" record a project has, and no medium covers it (web is build-only by operator decision; the five medium types have no forge shape). A field-side exporter script would rot privately and violates the field program's standing no-workaround rule for generic gaps; going straight to a forge medium would answer its five design questions (anchor namespace, change signal, enumeration cost under rate limits, artifact granularity, intent-scoping fields) from speculation instead of pilot evidence.

## Consequences
Any software mem can now bind its issue archive through a mirror, and the forge-medium design questions get answered from running mirrors rather than speculation. The honest cost is a freshness gap the pilot cannot close and the tool states at its point of use (its own surface and the mirror's generated README): the mirror is exactly as fresh as its last run — the engine measures mem-vs-mirror, nothing measures mirror-vs-GitHub; only a real forge medium with its own change signal closes that link. Incremental runs honour an updatedAt watermark and never prune (their fetch window cannot distinguish gone from unchanged); `--full` refetches and prunes.

## Options

Rejected: a field-side script (rots privately, duplicates per field program, violates the no-workaround rule). Rejected: going straight to a forge medium (the five design questions are cheaper to answer from a running mirror). Rejected: a stable CLI subcommand now (a product surface stamped before the medium design would constrain the design or break its consumers when the real medium lands).

## Notes


