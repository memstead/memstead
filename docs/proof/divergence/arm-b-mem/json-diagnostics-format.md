---
type: contract
created_date: 2026-07-15T07:30:49Z
last_modified: 2026-07-15T08:43:01Z
protocol: other
version: 0.1.0-pre
stable_since: 2026-07-01
deprecation_status: draft
tags: diagnostics, json, ai-first
---

# JSON Diagnostics Format

## Summary
Kāra's machine-readable compiler-diagnostics and error-trace surface: structured JSON diagnostics emitted alongside human text, and a runtime error-trace printer whose format is selectable via `KARAC_ERROR_TRACE_FORMAT`. The concrete wire form of the AI-first compiler interface.

## Relationships
- **REFERENCES**: [[ai-first-compiler-interface]]
- **REFERENCES**: [[diagnostics-system]]

## Request Shape

Configuration, not a request body:
```
KARAC_ERROR_TRACE_FORMAT=json | jsonl | text   # atexit error-trace printer
```
Compile-time diagnostics are produced by the compiler run; the structured channel is emitted in addition to the human-readable stream.

## Response Shape

- Diagnostics as structured JSON records with codes (e.g. E0236, W0237), source spans, and machine-applicable suggestions.
- Error traces printed at exit in `json`, `jsonl`, or `text` per the env var.
- `dbg()` output is task-id tagged and structured.

- Each diagnostic carries a typed DiagnosticClass and, on type errors, typed expected/got fields plus a `class` tag (`karac explain --format=json --class=...`).
- Machine-applicable fixes ride a `fixes[]` array on the JSON diagnostic.
- Stub hints (for missing test-referenced functions) are emitted as a `hints[].diff` JSON envelope, with literal-argument inference for the stub signature.

## Errors

Not applicable — this contract IS the error/diagnostic surface. Diagnostic codes are the payload (parse/resolve/type/effect/ownership families, lint codes).

## Versioning

Pre-1.0; codes and JSON shape evolve with the [[diagnostics-system]]. Format selected per-run via env var.

## Deprecation



## Notes

Realizes the [[ai-first-compiler-interface]] principle at the wire level. Consumers include agent tooling such as the Mend harness.
