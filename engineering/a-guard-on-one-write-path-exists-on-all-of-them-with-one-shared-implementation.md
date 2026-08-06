---
type: principle
created_date: 2026-08-06T07:52:39Z
last_modified: 2026-08-06T07:52:39Z
authority: accepted
universality: domain-wide
tags: write-gates, parity, validation, mutation-surface
---

# A guard on one write path exists on all of them with one shared implementation

## Statement
The legality of an operation never depends on which verb performed it. When a validation gate exists on one write path, every sibling path that can express the same operation runs the SAME gate — one owning implementation, called from every verb, never per-path copies. New gates land in the shared funnel first; a gate that cannot be shared is a design smell to resolve, not a licence to fork.

## Scope
Every engine mutation surface where multiple verbs express one operation class: edge creation (relate, create.relations[], update.declare_relations, batch paths), metadata write gates (create/update set paths), and the lifecycle setter family (capability gates). Applies equally to future verbs — a family-level test that enumerates the siblings is the enforcement of choice, so an ungated newcomer fails a test rather than shipping a hole.

## Relationships
- **REFERENCES**: [[never-silently-admit-unvalidated-content-into-the-graph]]

## Justification

Verified twice in one sweep (2026-08-06): the cycle family (self-loop + acyclic-cycle refusals) ran only on relate, so the identical illegal edge written through create.relations[] or update.declare_relations landed on disk and was silently dropped by the next boot's sweep — with the sweep's own coverage comment claiming the hole was closed; and `set_mem_schema` was the one lifecycle setter without the read-only-mount gate its six siblings carried, making a schema-pin change (a migration trigger) the one mutation a sealed mount could not refuse. Earlier instances of the same class: the create-path reserved-metadata-key asymmetry and the pre-anchor-merge validation gaps. Per-path copies are how this class of bug is born — the drift between the store-builder comment and reality proved the copies rot independently. Complements [[never-silently-admit-unvalidated-content-into-the-graph]]: that principle demands a gate at every boundary; this one demands the gates be the same gate.

## Exceptions



## Consequences

Behaviour tightenings are the expected cost: closing a parity hole makes writes refuse that previously landed (and were then silently corrected or left inconsistent). Refusal complements must prove no legal write regresses. Boot-time sweeps demote to last-resort nets for pre-existing data — their population can only shrink once write paths are closed.
