---
type: decision
created_date: 2026-09-05T17:24:10Z
last_modified: 2026-09-08T21:08:56Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes and delegated schema-change rules
scope: subsystem
tags: due, schema, planning, health
---

# A due axis counts from an engine-stamped timestamp with an offset and waits for its roster, and the bundle overstay is such a due date

## Decision
The schema-declared due axis gains three generic capabilities: date_field may name an engine-stamped timestamp (created_date, last_modified) that no type declares, so a deadline can be N days after the last change without an author writing a date; offset_days moves the due date past the field's date (an archive window, a review interval, a grace period); and unless_open_via holds an entity back while any related entity is still open (relationships, direction, the neighbour's status_field and open_values), so a container is not due while a member is. memstead health --include due serves the due brief as data over the default 90-day window, the same rows memstead due renders, with a Due section in the markdown; both take the same hidden --today hook so a recorded rendering pins the day. The workspace-local planning schema's bundle type declares its archive as due: date_field last_modified, offset_days 14, open_values [complete], and unless_open_via PART_OF incoming with the plan's open values, so a complete bundle left mounted appears on memstead due fourteen days after its last change and never while a plan is open; the graph-plan walker's completion step reads that instead of a script. The plan-criteria lint retired without an engine successor: its wire-code rule is knowledge of this engine's error index, which no other planning mem shares, and its constraints rule is semantic; the walker's grader carries the class and the walker's enforcement map records the retired row.

## Context
Two workspace scripts guarded the plan-mem lifecycle from outside the engine: one exported every plan mem to find complete bundles older than a grace period, one linted a bundle's acceptance criteria against the docs-site error index before the first session. The 2026-09-04 census (posture C) asked each to move into the engine or retire. The engine already owned a due axis (a schema declares which fields carry deadline semantics and the due brief renders what is due), so the overstay was one more schema-declared date once the vocabulary could count from a timestamp and wait for a roster; the lint was engine knowledge no other planning mem shares, so it retired to the grader that already re-derives every criterion from the error index and the plan's constraints. Rejected: moving the criteria lint into memstead gates (a planning mem about any other software would get false findings or need configuration; the 2026-07-07 rule forbids a one-off in the engine), a separate due generation of the planning schema (the engine accepted the declaration on the existing 0.6.0 package, re-installed), and a new memstead verb for the overstay (the due brief already answers the question).

## Consequences
Any schema can declare a deadline relative to an entity's own timestamps and gate it on a roster; the planning schema's six mounted mems declare the overstay reading today and none is overdue. Two scripts and a fixture set retired; the walker lost its step 0 and gained an enforcement-map row naming the grader as the carrier, with the 2026-09-02 evidence-engine cost that had produced the lint. The health include vocabulary grew by due (coverage: advisory, a reading never a verdict), the CLI reference and the MCP tool description name it, and the type-definition meta-schema carries the new due keys. Left open: an engine-side reading of the state the roster gate depends on is only as honest as the members' status fields, which is the planning schema's own discipline.

## Relationships
- **GOVERNS**: [[engine:due-brief]]
- **MOTIVATED_BY**: [[a-schema-declares-what-is-due-and-what-would-resolve-an-open-entity-and-health-reads-both]]
