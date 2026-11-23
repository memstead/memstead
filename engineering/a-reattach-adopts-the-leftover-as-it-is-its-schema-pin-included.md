---
type: decision
created_date: 2026-11-23T13:02:32Z
last_modified: 2026-11-23T14:40:19Z
status: accepted
decided_on: 2026-11-23
deciders: operator (backlog bundle, go of 2026-11-23), implementing agent
scope: subsystem
tags: mem-lifecycle,reattach,schema-pin,allowlist,recovery
---

# A reattach adopts the leftover as it is, its schema pin included

## Decision
We chose to have a reattach take the leftover mem as it is: when a create routes to a reattach (named with `recovery: reattach` or `--reattach`, or defaulted by the tombstone a `mem unregister` left), the mount's schema pin is the leftover's own, read from its config before the create allowlist judges the call, and the reply names that pin. The schema the call names is not applied to a mem that already exists. An overwrite and a fresh create take the named schema as before.

## Context
A reattach recorded the schema the call named as the mount's pin. The CLI names `default@1.3.0` when no `--schema` is given, so a leftover of another schema came back with a reply that named the default, a mount that contradicted the mem's own pin (`SCHEMA_PIN_MISMATCH` under the strict referee), and a refusal `MEM_SCHEMA_NOT_ALLOWED` where the create allowlist names only the mem's real schema. A grade found it when the delete warning's way out, `memstead mem init <name> --reattach`, was refused for such a mem.

## Consequences
- The way out that the delete warning and the residue refusal print works as written for a mem of any schema that resolves.
- The create allowlist judges the schema the mem carries, not the one the call names.
- A schema named beside a reattach is ignored and no warning says so; the reply's schema line is the statement.
- A leftover whose pin does not resolve is not reattached: the create refuses `SCHEMA_NOT_FOUND` where the pin is read, before the allowlist and before anything is written, so the tombstone of an unregister stays. The refusal names the schema install, as a fork's does, since a reattach keeps the pin it finds; it no longer advises a repin, which cannot reach a name that is off the roster (until 2026-11-23 the attach refused it, after the tombstone was cleared).
- One more read of the leftover's config per create that names a reattach or meets a leftover.

## Relationships
- **INFORMED_BY**: [[a-write-that-meets-a-pack-is-done-again-refs-are-read-so-that-they-stay-current-and-a-step-a-pack-disturbed-is-repeated]]

## Options

- Name `--schema` in the warning's command: rejected, the caller would have to know a pin the engine can read, and the wrong pin after an unregister would stay.
- Refuse a reattach whose named schema differs from the leftover's: rejected, the CLI always names one, so the plain command would refuse for every other schema.
- Make the CLI's `--schema` optional and look the pin up there: rejected, the MCP tool would keep the defect.
- Adopt the leftover's pin in the engine, before the allowlist: chosen.
