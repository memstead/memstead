---
type: decision
created_date: 2026-09-02T20:10:20Z
last_modified: 2026-09-02T20:10:20Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: export, provenance, redaction, leak-scan
---

# Export redaction speaks the leak-scan vocabulary and reports what it redacted

## Decision
A mem archive export redacts the provenance it carries with the seven scan lines of the leak scan (`scripts/leak-scan.sh`) as its one vocabulary, replacing each match with a `[redacted:<class>]` sentinel, and the export result reports the redaction count per class on the CLI and the MCP surface alike. A test parses the scan script and refuses when the engine's vocabulary drifts from it.

## Context
Bundle B plan 1 (2026-09-02). The memstead.ai seal carried an allowlist line excusing private patterns that rode into the published archives inside commit provenance; the engine had no redaction seam, so the leak scan and the export disagreed about what may leave the machine.

## Consequences
One vocabulary, two consumers: the leak scan and the export refuse the same classes, and the seal's allowlist lost its two provenance lines. An archive that redacted something says so in its result rather than passing as clean.

## Options

A second, engine-owned pattern list rejected: it would drift from the scan the release gate runs. Stripping provenance wholesale rejected: the counts and classes are the trust surface a reader keeps.
