---
type: principle
created_date: 2026-08-07T09:09:07Z
last_modified: 2026-08-07T09:09:07Z
authority: accepted
universality: domain-wide
tags: design, architecture, special-cases, unification
---

# Unify rather than patch

## Statement
When a special case and a general mechanism both exist for the same job, the special case is dissolved into the general mechanism rather than patched alongside it. The test: a proposed change that extends or accommodates a structural special case must first ask whether folding the special case into the general concept is cheaper over the system's life — and when it is, unification is the change.

## Scope
Project-wide engine and surface design (operator decision 2026-08-06, agent-toolbox bundle decision 1). Applies whenever work touches a structural special case that shadows an existing general mechanism — storage and mount models, tool and command surfaces, schema vocabulary, lifecycle verbs.

## Relationships
- **GOVERNS**: [[engine:mount]]
- **REFERENCES**: [[engine:mount]]
- **REFERENCES**: [[engine:read-mem-install-and-cache-pipeline]]

## Justification

A structural special case gets more expensive to keep than to fold: every new capability must be taught about it separately, and its divergence compounds. The instantiating example is the read-mem attachment model: read-mems' host-mem attachment was the one structural special case in an otherwise clean mount concept, and rather than adding a minimal uninstall verb beside it (the patch), it was dissolved into ordinary read-only [[engine--mount]]s with a symmetric uninstall — the [[engine--read-mem-install-and-cache-pipeline]] now materialises registrations as plain mounts. The principle is kin to the standing 2026-07-07 engine-change directive (changes must be generically useful, never one-offs specialized for the immediate need): that rule prevents new special cases at the door; this one retires the special cases already inside.

## Exceptions

- Sealed vocabulary and shipped meaning: unification never licenses changing semantics under a stable name — `propagating_relationships` was deprecated beside a new, honestly-named declaration rather than silently repurposed, because sealed schemas in the wild rely on the shipped meaning.
- When the general mechanism does not exist yet, dissolving the special case means building the general mechanism first — not deleting the special case and leaving the job uncovered.

## Consequences

- Fold-versus-patch is argued explicitly in plans and trade-off sections; "keep the special case, add a small verb" needs a positive argument, not the default.
- Dissolutions are breaking changes by nature and are acceptable pre-1.0; the cost is paid once at the fold instead of indefinitely at every future touch point.
