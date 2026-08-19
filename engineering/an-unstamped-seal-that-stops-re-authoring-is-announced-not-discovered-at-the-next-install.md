---
type: decision
created_date: 2026-08-19T07:02:56Z
last_modified: 2026-08-19T07:02:56Z
status: accepted
decided_on: 2026-08-19
deciders: backlog-sweep plan 06 (operator decision 19, 2026-08-18)
scope: subsystem
tags: schema-lifecycle, health, provenance, rot
---

# An unstamped seal that stops re-authoring is announced, not discovered at the next install

## Decision
Health carries a low-tier rot axis for UNSTAMPED schema pins: when a sealed package's content no longer passes current-language authoring validation (retired keys such as `propagating_relationships`), the report surfaces `SCHEMA_UNSTAMPED_SOURCE_ROT` naming the condition and the remedy — re-author under the current language and `memstead schema install` it, which re-seals AND stamps, handing future drift to the divergence axis. The stamped-divergence axis is untouched: its no-false-positive contract stands, and stamped pins are never double-reported. An unstamped package that still parses under the authoring tier produces no hint. Automatic backfill-stamping was rejected — stamping rewrites what the divergence axis checks, silently converting unchecked into checked-against-an-assumption.

## Context
Field observation (plenum-agent test, 2026-08-09): anker's grounding package still carried the retired key, so it was no longer installable — while its mem ran fine for weeks on the tolerantly-loaded seal, and nothing said so until the next install attempt. The tolerant-seal doctrine ([[a-seal-carries-its-sources-generation-and-never-asserts-one]]) is what makes this silence possible: sealed reading deliberately survives language changes, so only an explicit diagnosis can reveal that the AUTHORING path has rotted.

## Consequences
A holding can no longer run for months on a seal whose source is unrecoverable as a package without hearing about it. The probe is read-only and backend-uniform: folder seal directories go through the strict directory loader; git-branch seals are re-read from the registry ref and checked by a new in-memory authoring-tier validator (`check_package_reauthorable`), with the file list reconstructed from the pinned schema's own type roster — no new backend surface was added.

## Relationships
- **REFERENCES**: [[a-seal-carries-its-sources-generation-and-never-asserts-one]]

## Options



## Notes


