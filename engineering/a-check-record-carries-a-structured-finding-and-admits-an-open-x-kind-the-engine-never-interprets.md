---
type: decision
created_date: 2026-09-02T03:03:02Z
last_modified: 2026-09-02T03:03:02Z
status: accepted
decided_on: 2026-09-02
deciders: execute-graph-plan loop, evidence-engine bundle
scope: subsystem
tags: checks, ledger, health, agent-trust, vocabulary
---

# A check record carries a structured finding and admits an open x- kind the engine never interprets

## Decision
We chose to let a check record say WHAT it found, in a locatable form, and to let a checker from outside the engine's vocabulary declare its own kind without the engine pretending to understand it. A check (`memstead check`, single and `--from`, and `memstead_check`) accepts an optional `finding {code, message, section?, evidence?}`: `code` is the checker's own vocabulary, the wrapper shape is fixed, and a missing `code` or `message`, an empty value or an unknown key refuses `INVALID_CHECK_FINDING` naming the shape before anything is appended. The finding is persisted on the ledger line (serde-default, so every existing line still parses and finding-less lines stay byte-identical), echoed on the response, and rendered by the health checks axis under the entity's latest verdict. A kind of the form `x-<name>` (lowercase letters, digits, hyphens) is accepted and recorded verbatim: the engine aggregates only its own two kinds, `verification` and `conformance`; a foreign kind stamps no schema pin, moves no `check_state`, and appears on the checks axis only as a count per kind. Any other unknown kind keeps refusing `INVALID_CHECK_KIND`, with the vocabulary named. The independence derivation is unchanged: identities only, as recorded, and a finding never influences it.

## Context
The check ledger recorded a verdict, a method note, the entity hash and the caller's role and identity, with a closed two-kind vocabulary whose module comment said a third engine kind is a separate decision. Two needs arrived together with the investigative evidence mems. First, a `failed` verdict with only a free-text method note forces the author to re-derive the failure: the auditor knew which step hid a premise and had nowhere structured to say so. Second, the operator intends to run a checker from another model family against the pilot stock through this surface, and that checker's judgments are neither `verification` nor `conformance`; forcing them into either would make the engine's state derivation assert something it never checked, and letting them into an unprefixed open vocabulary would let a typo of an engine kind silently become a new kind.

## Consequences
- A failed check now carries where and why; the health checks axis shows it under the entity's latest verdict, so the repair does not start from zero.
- Foreign checkers get a recorded, listed, non-interpreted place in the ledger; the engine's two derivations stay exactly as they were, and a foreign `ok` never turns a failed verification green.
- The ledger stays append-only JSONL with no migration: old lines parse, new fields are optional.
- Two vocabularies are open by design (`finding.code`, the `x-` name) and everything the engine interprets stays closed: verdicts, engine kinds, the wrapper shape.
- A legacy line with an unrecognised, unprefixed kind keeps its historical reading as `verification`; only the `x-` prefix opts out of aggregation.
- The response's `check_state` for a foreign-kind record reports the entity's verification state, which that record did not move.

## Relationships
- **INFORMED_BY**: [[one-name-per-concept-across-every-surface-and-a-retired-name-refuses]]

## Options

- Free-text `method` only: rejected, a failed verdict with no locatable reason forces the author to re-derive the failure.
- An open kind vocabulary without a prefix: rejected, a typo would silently create a new kind; the `x-` prefix makes the declaration deliberate, mirroring the rule that a third engine kind is a separate decision.
- Findings as entities in a process mem: rejected for this bundle, the check is about the entity's hash at a moment and the ledger is where that moment lives; process-mem findings remain available for durable follow-ups.
- Interpreting foreign kinds as `verification`: rejected, the engine would assert a state it never derived.
- Chosen: a fixed finding wrapper with an open code, an `x-` kind recorded verbatim and counted, two engine kinds unchanged.

## Notes

Landed in the engine's 0.15.0 line: `check.rs` (`CheckFinding`, `RecordKind`, the `finding` field, `resolved_kind` returning `None` for foreign kinds), `record_check_with`, the health checks axis (`findings`, `foreign_kinds`), the CLI `--finding` flag and batch entry field, the `memstead_check` `finding` parameter on both MCP flavours, the changelog and regenerated references. Tests cover the shape validation, the kind grammar, the pre-finding ledger fixture, the foreign kind moving no state, and the conformance pin.
