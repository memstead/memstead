---
type: decision
created_date: 2026-08-08T22:54:36Z
last_modified: 2026-08-08T22:54:36Z
status: accepted
decided_on: 2026-08-09
deciders: operator (agent-trust plan 13)
scope: subsystem
tags: provenance, roles, trust-model, trailer, append-only
---

# Mutation provenance records caller-declared roles tamper-evidently in append-only history

## Decision
Every mutation records the caller-declared ROLE it was performed in, from the closed vocabulary `author` | `checker` | `verifier` | unspecified, immutably where history is append-only: the `Role:` commit trailer on git-branch mems, a `role` field in the folder JSONL ledger — one shape across backends, extending the existing `Actor:`/`Client:` trailer mechanism rather than inventing a parallel one. Declaration is per call (an optional `role` parameter on every MCP mutation verb) or per session (the `--role` flag on both binaries — the memstead CLI invocation and the memstead-mcp server instance), per-call winning; the role travels as engine session state (the mutation-clock precedent — the surface sets it before every mutation, so no mutation signature widened). The trust model: roles are CALLER-DECLARED but TAMPER-EVIDENT — bound to specific operations in append-only history, they cannot be edited after the fact and identities can be cross-checked across operations, which no self-written metadata field can provide. `unspecified` is legal forever: never refused, recorded as absence (no trailer, no ledger field — old records read back identically), and downstream gates treat it as cannot-confirm, never as any specific role. An unknown role value refuses typed (`INVALID_ROLE`) naming the declarable vocabulary on every surface.

## Context
Two field projects independently grew process bookkeeping (`checked_by`, `found_via`, `method`) as metadata fields on subject schemas — self-maintained, forgettable, and fakeable, because a field an agent writes is a claim, not a record. The bundle's recorded decision made the sharper cut: the ENGINE records who performed every mutation and in what declared role, at the moment it happens; process state then becomes something derived from records, not maintained in fields. The trailer/ledger mechanism already carried actor and client identity per mutation ([[engine--per-mem-commit-and-provenance-trailer-layer]]); the role dimension rides the same rails. The gate value — the author≠checker axis — comes from comparing recorded identities across operations.

## Consequences
The recording substrate for check operations exists: "who created this, who checked it, were they different" becomes derivable from history instead of trusted from fields. Subject schemas carrying process fields keep working (their retirement is their authors' choice once the gating tier exists) — nothing breaks. Costs accepted: role is engine session state set by the surface before each mutation — a new surface that forgets to set it records unspecified (fail-honest, not fail-wrong); recording precedes any enforcement deliberately (recording first, gating later on evidence — the sync-ab way); and cryptographic attestation is layered work for later under the same recorded shape, not a prerequisite.

## Relationships
- **REFERENCES**: [[engine:per-mem-commit-and-provenance-trailer-layer]]

## Options

Roles as schema metadata fields — rejected: self-report, mutable, per-schema reinvention. Deriving role from tool identity (CLI = author) — rejected: role is a property of the WORK, not the transport; one session legitimately authors in one mem and checks another. A full principal/auth system — rejected as premature: tamper-evidence plus cross-operation comparison covers the pre-1.0 gates. A new provenance tool — rejected: tool-count policy; the entity read will serve the derived block.

## Notes


