---
type: decision
created_date: 2026-11-24T09:43:19Z
last_modified: 2026-11-24T09:56:23Z
status: accepted
decided_on: 2026-11-24
deciders: operator (next operation over reload, 2026-11-24), implementing agent
scope: system
tags: schemas,mounts,views,drift
---

# A running engine follows a schema pin moved where the mem lives, and every reply names the configuration's pin

## Decision
We chose that a running engine follows a schema pin moved where a mem lives at its next operation, at the same check that already follows a moved head, and that one reading in the engine names a mem's pin for every reply: the pin in the mem's configuration, the roster row's only where the configuration states none. The check compares a configuration cursor (for a branch mount the tip of the git directory's schema-and-config ref, for a folder mem a fingerprint of its configuration file) with the one the engine last saw, reads the configuration only when it moved, and acts only when the pin moved. A pin that does not resolve takes the mem out of service, as a start would. The roster row of the following workspace is not rewritten.

## Context
A mem can be mounted by the workspace that owns its git directory and by others (see [[engineering--serve-callers-with-different-rights-through-one-view-per-set-of-rights-a-view-takes-the-schemas-its-mounts-pin]]). The pin lives with the mem; each mounting workspace's roster row carries a copy. When the owner moved the pin, the other workspaces' rosters did not change, and measured on 0.27.1 and 0.28.0 an engine already running there kept validating writes against the old schema through every reload until restarted. After the restart several replies still named the roster row's old pin while the engine served the new one, and a row without a pin made the schema tool refuse for an empty name. The pre-operation check compared only a mem's head, which a pin move does not touch. The operator chose the next operation over a reload on 2026-11-24, on the recommendation that a host with many such workspaces would otherwise have to restart every engine after each pin move.

## Consequences
- No restart and no reload is needed after a pin move; a reload asked for follows the pin too.
- The price is one cursor comparison per mem per operation; the configuration is read only when the cursor moved. A version, title, description, subject or sync state set on the mem, a schema installed beside it, or another mem's pin in the same git directory moves the cursor and touches nothing else.
- A migration a workspace started itself keeps its target; a migration target set by another workspace lives on that workspace's roster and is not followed.
- The roster row stays the workspace's own statement and a cross-check; the health read reports the difference as `SCHEMA_PIN_MISMATCH`.
- A mem quarantined by an unresolvable moved pin stays out of service until a reload or a write of this workspace's roster (which tries every quarantined mount again), also when the owner moves the pin back.
- A reload of one mem reads the schema sources again when its pin is not among the schemas the engine holds, so a mem whose schema was installed since the start comes back without the full form.
- A failed configuration probe marks the mem's view unsure like a failed head probe, so no write lands on it.

## Relationships
- **REFERENCES**: [[serve-callers-with-different-rights-through-one-view-per-set-of-rights-a-view-takes-the-schemas-its-mounts-pin]]

## Options

- Follow at the next operation: chosen.
- Follow only at a reload: rejected by the operator. Until someone reaches every running engine, they write entities the new schema refuses.
- Read the configuration on every operation: rejected. The cursor answers that nothing moved without reading a tree.
- Rewrite the following workspace's roster row to the new pin: rejected. A row rewritten by the engine on another workspace's act is no cross-check, and the row's identity includes the pin, so the write would itself look like a mount change.
- Fix each reply that named the roster row's pin on its own: rejected. One reading, so a reply added later cannot pick the wrong source.
- A notice that tells callers the schema changed: rejected under the surface freeze; the replies name the pin.
