---
type: decision
created_date: 2026-08-06T07:16:41Z
last_modified: 2026-08-06T07:16:41Z
status: accepted
decided_on: 2026-08-06
deciders: operator (stability-sweep plan 02), implementing agent
scope: subsystem
tags: metadata, reserved-keys, write-gates, schema-loader, repair
---

# Reserve the identity metadata triple everywhere with unset as the sanctioned repair

## Decision
The entity identity/discriminator metadata triple `type` / `mem` / `id` is reserved with one behaviour on every path. The schema loader's reserved set widens from `type` alone to the full triple, enforced on the authoring/installation path (schema install, strict validation) with the typed `ReservedSchemaKey` refusal — and moved OUT of the boot path: a schema already sealed that violates the rule keeps loading, matching the heading-round-trip posture (refusing at boot would brick the workspace). The create path refuses a caller-supplied reserved key with the same deliberate `READ_ONLY_FIELD` the update path uses, replacing the incidental `UNKNOWN_METADATA_FIELD` refusal that only worked because no schema could declare the key. `metadata_unset` MAY name a reserved key: removal is the sanctioned repair for an entity that acquired a smuggled key before the write gates closed, and unsetting `type` re-seeds the authoritative discriminator from the entity's own type so the frontmatter can never go typeless (on a healthy entity the unset is a no-op). Setting a reserved key stays refused everywhere; create's stamp-and-proceed posture for engine-managed timestamp fields is untouched. Realized in the [[engine--schema]] loader and the [[engine--create-mutation]] / [[engine--update-mutation]] paths.

## Context
The backlog carried this as "a `type` metadata key smuggles past create validation and bricks the entity" (institute finding, 2026-07-16). The headline exploit had already been closed by two newer gates, but the residue was asymmetric: the loader reserved only `type` (a schema declaring `mem` or `id` loaded cleanly, after which create accepted those keys while update refused them — two paths disagreeing about a schema the loader was happy to install); the create-path refusal was incidental rather than deliberate; and because the update path ran the read-only check over `metadata_unset` too, an entity bricked before the gates closed had no sanctioned repair — only delete-and-recreate, which destroys provenance and edges.

## Consequences
One reservation, one behaviour: the refusal an agent sees names the real reason (`READ_ONLY_FIELD`, not `UNKNOWN_METADATA_FIELD`), no installable schema can declare an identity key, and the two write paths can no longer disagree. Historically bricked entities have an auditable per-entity repair route that preserves provenance and edges. The boot-path loosening is deliberate: enforcement moved from load-time to install-time, so pre-widening sealed schemas keep booting — the write-path refusals keep the reserved keys unwritable regardless. The loader records each type's raw pre-merge declared metadata keys (`declared_metadata_keys`) to make the install check possible without false-positives on engine-injected base fields.

## Relationships
- **REFERENCES**: [[engine:schema]]
- **REFERENCES**: [[engine:create-mutation]]
- **REFERENCES**: [[engine:update-mutation]]

## Options

**Widen the loader reservation vs. rely on the incidental refusal** — rejected: `mem`/`id` were one schema declaration away from being writable at create and refused at update. **Allow reserved-key unset vs. keep removal refused** — keeping refusal is symmetric but leaves bricked entities with delete-and-recreate as the only repair; removing a reserved key can only move an entity toward the invariant (with the `type` re-seed closing the typeless-entity hole the naive unset would open, since a missing `type:` silently re-types to the mem default on the next parse). **Automatic repair sweep** — rejected: touching entities nobody asked about is a data-touching decision; the manual route is auditable per entity.

## Notes


