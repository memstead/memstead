---
type: decision
created_date: 2026-08-28T11:27:24Z
last_modified: 2026-08-28T11:27:24Z
status: accepted
decided_on: 2026-08-28
deciders: operator (agent-trust plan 15)
scope: subsystem
tags: provenance, identity, checks, trust
---

# Caller-declared identity is the independence gate's only comparator

## Decision
Every mutation and every check can record a caller-declared IDENTITY — an opaque caller-chosen string (an agent name, a session handle, a person's tag) — immutably beside actor/client/role, on the same rails as [[mutation-provenance-records-caller-declared-roles-tamper-evidently-in-append-only-history]]: the `Identity:` commit trailer on git-branch mems, an `identity` field on the folder JSONL ledger and the check ledger, one shape across backends. Declaration follows the role pattern exactly: per-call `identity` parameter on every MCP mutation and check verb, session default via `--identity` on both binaries plus the `MEMSTEAD_IDENTITY` environment variable, per-call wins over flag wins over environment; the value travels as engine session state. The author≠checker independence gate compares IDENTITIES AND NOTHING ELSE: both records carry one and they are equal — `self_checked`; both carry one and they differ — `confirmed_independent`; either lacks one — `unconfirmable`, never a guessed category. The `(actor, client)` transport pair stays recorded as context and is never again a comparator. The engine neither generates, interprets, nor enriches the value (no PII posture change); over-length declarations refuse typed (`INVALID_IDENTITY`, cap 128 chars); absence is legal forever and historical records stay `unconfirmable` with no backfill.

## Context
The 2026-08-09 field test of the checks axis found the gate measuring the transport, not the actor: recorded identity was the `(actor, client)` pair, so everything written and checked through the CLI read `self_checked` regardless of who acted, and a cross-surface author/check read `confirmed_independent` — independence by accident of transport. The same-day interim fix made the gate honest but empty (every comparison `unconfirmable`). The field pattern the gate must serve is anker's: author and checker are different agent sessions, and that difference is the entire value of the check.

## Consequences
The gate [[check-state-is-derived-from-engine-recorded-check-acts-never-stamped]] feeds can finally fire: a consistent multi-agent setup gets real independence signals, and a lying setup defeats only itself — the identity is an honesty device, not authentication (cryptographic attestation stays rejected per the plan-13 decision). "Author and checker are different parties" now has a mechanical meaning, unblocking the run-brief roles prose on evidence. Rejected on the way: deriving identity from the MCP clientInfo handshake (names the software, not the actor), a session-minted random identity (fabricates a distinction the caller never claimed), overloading the role field (role and identity are orthogonal recorded dimensions), and waiting for a real principal system (the honest interim was a placeholder, not a resting state).

## Relationships
- **DERIVED_FROM**: [[mutation-provenance-records-caller-declared-roles-tamper-evidently-in-append-only-history]]
- **REFERENCES**: [[check-state-is-derived-from-engine-recorded-check-acts-never-stamped]]
- **REFERENCES**: [[mutation-provenance-records-caller-declared-roles-tamper-evidently-in-append-only-history]]

## Options



## Notes


