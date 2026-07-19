---
type: principle
created_date: 2026-07-15T07:24:00Z
last_modified: 2026-07-15T08:42:02Z
authority: established
universality: domain-wide
tags: ai-first, diagnostics, design-principle
---

# AI-First Compiler Interface

## Statement
The compiler's machine-facing surfaces are designed for LLM agents as a first-class consumer: diagnostics are available as structured JSON, error traces are emitted in machine formats, a compiler query channel answers programmatic questions, and formatting is canonical so tools can rewrite code deterministically.

## Scope
Applies to every Kāra compiler output an automated tool consumes — diagnostics, error traces, the query API, and the canonical formatter. Does not govern the human-readable REPL prose or the mdBook documentation, which are designed separately for people.

## Relationships
- **GOVERNS**: [[diagnostics-system]]
- **GOVERNS**: [[json-diagnostics-format]]
- **GOVERNS**: [[karac-cli]]
- **GOVERNS**: [[compiler-query-api]]

## Justification

Kāra targets a world where AI agents write and repair code (the Mend demo shows an agent writing Kāra end-to-end). Agents need parseable, deterministic compiler output rather than prose meant for a terminal. The brainstorm series (v63 LLM compiler query channel) graduated this as durable direction.

## Exceptions

- Human-facing surfaces (REPL banners, the mdBook, `karac explain` prose pages) optimize for human readability, not machine parsing.

## Consequences

- Diagnostics ship as structured JSON alongside human text.
- `KARAC_ERROR_TRACE_FORMAT` supports `json`/`jsonl`/`text` for the atexit error-trace printer.
- The formatter must be canonical (idempotent) so agent rewrites are stable.
- Method-resolution and typo diagnostics carry suggestions machines can apply.
