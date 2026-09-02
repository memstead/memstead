---
type: decision
created_date: 2026-09-02T12:41:02Z
last_modified: 2026-09-02T12:41:02Z
status: accepted
decided_on: 2026-09-02
deciders: operator (backlog-engine bundle A, go of 2026-09-02), implementing agent
scope: subsystem
tags: anchors,health,integrity
---

# An unreadable anchors sidecar is a typed condition on every read surface, never zero rows

## Decision
A sidecar the engine cannot read (an unknown version, a truncated file, an IO fault, a retired state name) is one typed condition, `ANCHORS_SIDECAR_UNREADABLE`, carried on every surface that reads anchors: the mem-scoped verification (`sidecar_error`), the health anchors axis (`condition` beside the counts, whose zeros the population string then declares meaningless), the integrity findings (a consistency finding, strict), the entity read, and the fidelity report (`details.reason`). The verification vocabulary reads `resolves` for the resolving state, and every anchor figure is rendered beside the population it covers.

## Context
Before 2026-09-02 an unreadable sidecar degraded to no anchors everywhere except the binding-scoped fidelity report, so a mem verified clean over rows nobody had read. A strict health run passed over a mem it could not measure.

## Consequences
Under `--strict` the condition counts once, through the finding under `--include integrity` and through the axis under `--include anchors`, so a strict run never passes clean over an unmeasured mem. The MCP tool description names `resolves` but not the condition: the payload's `condition` key is self-describing and adding it broke the 2048-byte client cap on descriptions.

## Options

Zero rows with a warning line: rejected, a warning is not read by the code paths that count. A separate anchors verb for the condition: rejected, the condition belongs beside the counts it voids.
