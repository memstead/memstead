---
type: decision
created_date: 2026-09-02T02:05:26Z
last_modified: 2026-09-02T02:05:26Z
status: accepted
decided_on: 2026-09-02
deciders: execute-graph-plan loop, evidence-engine bundle
scope: subsystem
tags: mutation, retype, mcp, cli, provenance, validation
---

# Retype is its own verb: validate against the target type, re-check every edge, refuse report-all, never drop silently

## Decision
We chose to give a type change its own verb, `memstead retype <id> --type <target>` on the CLI and `memstead_retype` on both MCP flavours, instead of admitting `type` on `update` or routing authors through delete plus create. The entity's id, file path and every incoming edge stay. The existing sections and metadata are validated against the TARGET type with create's own validators and a report-all refusal: every unknown section, missing required section, unknown or invalid metadata value, unsatisfied block-tier constraint and shape-violating edge arrives in one envelope (`details.problems[]`, each with the wire code the same condition carries on create, update and relate), together with the target's declared sections, its catch-all and a proposed `section_map`. The envelope's code is `UNKNOWN_SECTION` whenever a section key misses (the map is the first thing to fix), otherwise the shared code of a single problem class, otherwise `RETYPE_REFUSED`. Every edge is re-checked in both directions: outgoing under the new source type, incoming from loaded referrers under the new target type, cross-mem edges through the referrer schema's declaration; referrers in a deferred (lazy) mem are enumerated through the mount's own backend, and a mem that cannot be enumerated refuses `RETYPE_REFERRER_UNPROBEABLE`. Nothing moves or disappears unannounced: `section_map` renames keys, `drop_metadata` lets go of fields the target does not declare, and an unmapped or undropped item refuses. One commit lands under the subject grammar `memstead: retype <id>` with the new `retype` provenance kind, the provenance record exposes the mutation verb, and the response states `checks_stale: true` with a note that check records and derivation baselines on the entity are stale because its content hash moved.

## Context
The identity triple (`mem` / `id` / `type`) is reserved on `update` by decision, and delete refuses an entity with incoming references. Between the two, an entity that turned out to be the wrong type had no legal path: delete plus create loses history, provenance and the `HAS_INCOMING_REFS` guard, and re-points every referrer by hand. The investigative evidence mems that motivated the bundle discover the anchor-versus-derivation boundary while writing, so the wrong first guess is the normal case, not the exception. Two facts shaped the design. First, the loader drops a shape-invalid edge at the next boot with only a `PARSED_RELATION_INVALID` warning, so a retype that skipped the edge re-check would amputate the graph on restart; the check on both directions is mandatory. Second, a spec's required `level` field cannot be unset and no other default type declares it, so the first honest attempt at `spec → memo` showed that a retype without an explicit way to drop a field is unusable for exactly the common case; the `drop_metadata` list is that way, and it stays explicit because dropping data unannounced is what the write gates exist to prevent.

## Consequences
- Authors migrate entities to the type they turn out to be, keeping id, path, incoming edges, history and provenance.
- The refusal is one envelope per attempt, so a second attempt can be the right one; a section is never moved into the catch-all and a field never dropped without being named.
- The identity triple stays reserved on `update`; `update --from` and the `memstead_update` metadata description point at the new verb.
- The MCP roster grows to 20 tools (14 lean), propagated to the tool-surface tests, the generated references, the handbook and the engine mem's tool-surface contract.
- Referrers in a mem that was removed from the roster after the edge was written are outside the engine's knowledge and are not re-checked; the risk is recorded on the plan that landed the verb.
- Check records and derivation baselines keyed to the previous hash go stale on every retype, by construction; the response says so and leaves re-checking to the author.
- The criterion that graded the edge re-check named a code that does not exist (`INVALID_RELATIONSHIP_SHAPE`); it was corrected to the engine's one name for the condition, `INVALID_REL_SHAPE`, rather than minting a second code.

## Relationships
- **INFORMED_BY**: [[schema-vocabulary-says-only-true-things-via-leaf-declaration-and-the-self-loop-rename]]

## Options

- Delete plus create with reference re-pointing: rejected, loses history and provenance, and `HAS_INCOMING_REFS` is a correct guard, not an obstacle.
- Allow `type` in `update` metadata: rejected, the reserved triple is a recorded decision and the operation needs its own validation and provenance kind.
- Skip the incoming-edge check because the id is stable: rejected, id stability says nothing about the referrer's rel-type pins, and the loader drops the edge at the next boot.
- Silent section carry-over into the catch-all: rejected, that is the silent loss the write gates prevent; the map is explicit.
- Infer which metadata to drop from the target's declarations: rejected, for the same reason; `drop_metadata` names each field.
- Envelope code always `RETYPE_REFUSED`: rejected, the single-class case should carry the code the same condition carries elsewhere; the mixed case, and only that, gets the umbrella.
- Chosen: the explicit verb over create's validators, edges re-checked both ways, report-all refusal, explicit map and drop list.

## Notes

Landed in the engine's 0.15.0 line with the CLI command, both MCP tools and descriptions, the tool-surface tests, the changelog entry, the regenerated references, the handbook counts and the engine mem's tool-surface contract and retype spec. Tests cover the success path with restart, the unknown-section refusal, both edge directions, cross-mem and lazy-mem referrers, report-all, and the dry run.
