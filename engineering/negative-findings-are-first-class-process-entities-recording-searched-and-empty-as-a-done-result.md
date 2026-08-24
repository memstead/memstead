---
type: decision
created_date: 2026-08-08T20:58:37Z
last_modified: 2026-08-23T14:48:19Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust plan 10)
scope: subsystem
tags: ingest-schema, negative-finding, process-tier, auditability
---

# Negative findings are first-class process entities recording searched-and-empty as a done result

## Decision
The ingest process schema carries a fourth type, `negative_finding` (ingest@0.5.0): what was sought, the search directions actually walked (`search_path` — the load-bearing section, because git records WHEN a finding was made but only the entry can record WHERE the search looked, and a later reader judges coverage from exactly that), and the empty result stated as a result. The type is leaf-declared (an edge-less finding is never an orphan) with the same optional wiki-link reach into the destination claim the absence bears on that the other entry types have. The `coverage_gap` boundary is drawn in BOTH types' prose — each names the other and the operational rule for choosing: a gap is work to DO (the source has material the destination lacks), a negative finding is work DONE (the source space holds nothing — keep off). Dates are deliberately not a metadata field; the search path is what needs recording because git cannot carry it.

## Context
The most valuable entities in a prior research holding were absence findings ("the state never measured the effect") — and they existed only because an operator explicitly ordered agents to record them; without that order the difference between "not searched" and "searched, empty" vanishes with the session, and a research holding stops being auditable. The engine's doctrine already endorsed absence as an honest state; what was missing was a place to put it. The process tier is where it belongs — the artifact describes the procedure's outcome, not the subject — and the built-in ingest schema is the process tier this project owns. No engine change was required or wanted: the type is authorable by hand from day one, so it does not wait for the binding-free process-mem work.

## Consequences
Completed searches persist across sessions: a later run reads its own question in `sought`, judges coverage from `search_path`, and skips a search already walked — or deliberately reruns it when the source space has grown, superseding the entry (the type's write_rules state the expiry posture). Agents meeting `coverage_gap` and `negative_finding` side by side get the choosing rule at write time from either type's guidance, cutting the filing-confusion failure mode the plan named. Costs accepted: another type in the process vocabulary (the hard-cut test in the schema's system_message grew a fourth bucket), and negative findings can go stale silently if runs never re-examine them — the open-questions surface that would routinely re-serve them is later work.

## Relationships
- **ENABLES**: [[a-mem-enumerates-its-own-unknowns-through-a-composed-open-questions-axis-that-computes-nothing-new]]

## Options

Widening `coverage_gap` with a status field (open vs searched-empty) — rejected: opposite operational meaning, opposite lifecycle; merging guarantees the filing confusion the schema must prevent. An engine-level findings-store record — rejected: a negative finding is knowledge with provenance and relationships, not run residue; it belongs in the graph where it can be linked, searched, and exported. A generic engine-blessed process-fields block on every schema — rejected per the bundle decision: declared process state is self-report; this artifact is genuine knowledge, correctly entity-shaped.

## Notes


