---
type: decision
created_date: 2026-08-06T08:31:01Z
last_modified: 2026-08-06T08:31:01Z
status: accepted
decided_on: 2026-08-06
deciders: operator (stability-sweep plan 04), implementing agent
scope: component
tags: health, projections, include-catalogue, typed-codes
---

# Health issue codes are structured fields and the config projection is an include key

## Decision
Two health-surface contracts. (1) A `HealthIssue` carries a machine-readable `code` (`MISSING`, `SECTION_HEADING_MISMATCH`, `UNDECLARED_RELATIONSHIP`, `INVALID_REL_SHAPE`) as a typed field beside `field` and `message` — never as a message-string prefix. Every projection that lists issues carries the code; the `missing_fields` projections add per-issue `{field, code, message}` additively beside the byte-identical legacy `missing` field-name array. The enumeration lives with the type, never re-derived per projection. (2) The workspace-config projection is an ordinary member of the health include catalogue (`config`), rendered by one shared implementation in `memstead-base` for every surface (MCP composer, CLI `--include config`); the pre-existing `include_config` boolean stays as a documented alias forever — identical effect, rendered once when both are passed. Realized in the [[engine--graph-health-report-surface]].

## Context
Both contracts close observed information loss. The issue-code condition was carried only in a message prefix; the `missing_fields` include projected issues down to bare field names, so `SECTION_HEADING_MISMATCH` (content present under a non-deriving heading) surfaced under a "missing" label — partially re-introducing the exact misdirection the distinct finding exists to prevent, with tests asserting `message.starts_with(...)`. The config projection was MCP-only because it shipped as a separate boolean parameter instead of a catalogue member — a structural parity gap the CLI's catalogue-driven `--include` could never cross.

## Consequences
A typed field survives every projection that carries issues at all — the same shape logic as the mutation-warning envelope. Message prefixes stay in the text for humans but are never load-bearing. New include-worthy projections join the catalogue, not a new boolean; existing booleans are grandfathered as aliases rather than broken. All additions are additive: consumers of the prior payloads (legacy `missing` array, `include_config`) keep working byte-identically. The UniFFI `HealthIssue` record carries the code string, so the macOS app can branch without parsing messages.

## Relationships
- **REFERENCES**: [[engine:graph-health-report-surface]]

## Options

**Structured `code` field vs. keeping the message prefix** — the prefix fails the moment a projection drops messages, which is precisely what happened. **`config` as catalogue key vs. leaving the boolean** — leaving it keeps permanent CLI blindness; a CLI-specific flag would fork the vocabulary. **Replacing the `missing` array with rich objects** — cleaner in isolation but a breaking response-shape change for information the additive form delivers in full.

## Notes


