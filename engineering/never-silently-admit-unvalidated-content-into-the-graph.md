---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:44:03Z
authority: established
universality: domain-wide
tags: invariant, validation, conformance, ingress, integrity, trust-boundary, engine
---

# Never silently admit unvalidated content into the graph

## Statement
Every path by which content becomes, or would become, live graph state validates that content against the mem's pinned schema and refuses non-conforming input at the boundary. The engine never tolerates, papers over, or silently adopts content that would not survive a write — invalid input is rejected with a typed error, never quietly accepted into graph state.

## Scope
Applies to every ingress boundary of the engine where external or pre-existing content crosses into graph state: single-entity mutations (create/update/relate/delete/rename), whole-archive ingress (registry install, publish, export re-pack), remote history transport (the pull and push schema gates), and mem bootstrap (the init/quickstart adoption posture). Does NOT govern read paths — the steady-state loader may tolerate parser drift and surface it as health findings rather than refusing, because a read never advances graph state and the same content is re-checked strictly at the next ingress.

## Relationships
- **REFERENCES**: [[engine:runtime-validator]]
- **REFERENCES**: [[engine:archive-ingress-validator]]
- **REFERENCES**: [[engine:read-time-integrity-linter]]
- **GOVERNS**: [[engine:runtime-validator]]
- **GOVERNS**: [[engine:archive-ingress-validator]]
- **GOVERNS**: [[engine:read-time-integrity-linter]]

## Justification

The engine is the single place where entity conformance and cross-mem integrity are enforced; if any ingress path admitted invalid content, the graph would corrupt silently and every downstream reader would inherit the corruption. Concrete surfaces share this posture: the [[engine--runtime-validator]] gates single-entity writes against the active schema; the [[engine--archive-ingress-validator]] is the single trust boundary every `.mem` archive crosses before a byte reaches a cache or a running engine; the [[engine--read-time-integrity-linter]] re-runs the same write-time conformance checks so read-time and write-time verdicts can never drift; the pull and push schema gates keep an invalid tree from ever landing on a branch the engine will later read.

## Exceptions

- Read paths tolerate: the steady-state loader deliberately uses a tolerant parser and reports drift as findings instead of refusing — safe because a read advances no state, and the tolerated content is re-validated strictly at its next ingress.
- `fetch` is deliberately ungated (V1): it advances only remote-tracking refs, which are not yet the branch the engine reads; schema validation runs on the subsequent pull.

## Consequences

- A new ingress path that tolerated invalid content — an adopt-on-init mode, an unvalidated archive install, a pull without a schema gate — is a violation visible across every surface, not a local convenience.
- There is exactly one conformance vocabulary: the read-time linter and the schema-migration / update-repair gates reuse the write-time validator helpers rather than inventing parallel checks, so a check added at write time propagates to every ingress at once.
