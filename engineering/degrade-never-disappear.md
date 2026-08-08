---
type: principle
created_date: 2026-08-08T08:05:08Z
last_modified: 2026-08-08T08:05:08Z
authority: accepted
universality: domain-wide
tags: availability, boot, quarantine, agent-trust
---

# Degrade never disappear

## Statement
A failure disables the smallest unit that actually failed — never the workspace. A mem-level boot failure (unresolvable or missing schema pin, backend instantiation or read failure) quarantines that mem: it is listed on overview and health with its typed reason and repair command, serves nothing until repaired (quarantine is not tolerance — no partial or best-effort data from a broken mem; honest absence beats partial truth), and returns to service via reload after the repair, without a process restart. Binding-level failures quarantine that binding's operations. No validation is weakened anywhere: everything that failed the boot still fails it — the blast radius shrinks, the judgment does not.

## Scope
Engine boot and every serving surface over it. Mem-level: the per-mount boot loop quarantines instead of aborting; every mem-lookup failure routes through one helper so a quarantined mem refuses `MEM_QUARANTINED` (carrying the underlying typed reason), never masquerading as `UNKNOWN_MEM`. Workspace-level failures that genuinely leave nothing loadable (an unparseable workspace store) remain fatal for mem serving — but typed, and on MCP answered by a diagnostic surface rather than a silent exit. The retained mount record is what reload re-attaches from.

## Relationships
- **REFERENCES**: [[repair-verbs-operate-below-boot]]
- **REFERENCES**: [[type-boot-failures-at-the-seam-with-one-shared-message-across-surfaces]]

## Justification

Memstead's core promise is that the data is always readable — markdown + git, no lock-in. The engine inverted that promise at the worst moment, twice in two days: one mem with a bad schema pin took thirteen mems and 9,100 entities offline (plenum, 2026-08-06/07), and one legacy projection config took fifteen mems offline (expertise, 2026-08-07); in both cases the MCP server exited without a word, leaving sessions holding dead tool grants. The isolation idiom already existed in the loader (per-file parse errors collect instead of failing boot; a round-trip-violating sealed schema degrades to a health warning) — this principle extends the proven pattern to pin resolution and mount instantiation. Best-effort serving of a broken mem was rejected: an agent reasoning over a silently partial mem produces confidently wrong conclusions; the failure must be visible at mem granularity.

## Exceptions



## Consequences

One broken artifact can no longer hold healthy mems hostage. Boot never mutates — auto-repair at boot stays rejected; repair is an explicit, logged act through [[engineering--repair-verbs-operate-below-boot]] verbs, and the quarantine roster's reason messages (typed per [[engineering--type-boot-failures-at-the-seam-with-one-shared-message-across-surfaces]]) name those commands. The roster rides the existing overview/health dashboard — no new MCP tool. The historical wholesale-abort regression tests are replaced by quarantine-behaviour tests as a deliberate act, not deleted in passing.
