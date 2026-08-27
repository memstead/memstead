---
type: principle
created_date: 2026-08-27T19:26:52Z
last_modified: 2026-08-27T21:59:30Z
authority: accepted
universality: domain-wide
tags: wire-surface, naming, mcp, cli, ui-api, migration, engine
---

# One name per concept across every surface, and a retired name refuses

## Statement
One concept carries one field name on every surface that speaks it, inputs and outputs alike; when a name is retired, the old one is refused rather than accepted as a synonym. Testable two ways: a grep for the retired name returns nothing outside history, and a call using it fails with a typed refusal rather than succeeding.

## Scope
Every machine-facing surface over the engine: the MCP tools on both flavours, the CLI (flag and batch-payload shapes alike), the private HTTP layer and the contract it generates, and the tool descriptions that teach any of them. Prose and generated docs are in scope because a name that survives only in an example is still a name an agent will copy. Out of scope: values (a `type` field naming an entity type or a content-block kind is a different concept that legitimately owns that word), and history — changelogs, protocols, run captures and superseded plans keep the spelling they were written with.

## Relationships
- **REFERENCES**: [[agent-first-surface-design]]
- **GOVERNS**: [[engine:mcp-tool-surface]]
- **GOVERNS**: [[engine:cli-command-surface]]

## Justification

The cost is paid by the reader, not the writer. A relation edge was spelled four ways depending on which door an agent came through, and `memstead_update` alone spoke two of them: `{to, type}` on its declaration list and `{rel_type, target}` on its unset list. A newcomer paid a refusal round-trip on exactly that in 2026-08. Documentation had already been tried as the fix, in a note warning that the sibling surfaces differ and telling the reader to read the shape at hand; a warning that a surface is confusing is not a fix for the confusion, and it is evidence for this rule rather than against it. The no-alias half is the part that looks unkind and is not. An alias is the cheapest migration and it makes the defect permanent: both spellings then live on in examples, transcripts and agent memory, and every future reader has to learn that two names mean one thing. Pre-1.0, a refusal costs one corrected call; an alias costs every reader forever. See [[engineering--agent-first-surface-design]] for whose reading is being optimised.

## Exceptions

Where a shape names both ends of a relation it uses the pair `from`/`to`; where the near end is implied by the call it names the far end `target`. That is not two names for one concept: the endpoint's role differs, and the pair form would be ambiguous without both. A surface may also keep a distinct serialization while sharing the vocabulary, as the CLI's `--relation REL_TYPE:target-id` joins the same two names with a colon.


A projection built to a FOREIGN consumer's contract keeps that consumer's names, and this exception is stated rather than assumed because the tree contains one: `serve/src/graph.rs`'s `GraphEdge` and `registry/src/serialize.rs` both serialize the relationship type as `type` with endpoints `source`/`target` for the shared graph.json payload, which speaks a force-graph front end's vocabulary and is documented in the registry mem as its own contract. It is not a mutation input, so it is not reachable by an agent trying to author an edge, which is the reading this rule protects.

The exception is deliberately narrower than it first read. `memstead-cli`'s `EdgeTypeCount` was listed here too, on the ground that an aggregate count row is not an edge a caller can write. That reasoning was too generous: nothing foreign consumes it, so keeping a second name cost a reader something and bought nothing. It was converged on 2026-08-27 and the exception withdrawn. An exception earns its place by naming a consumer whose contract would break, not by arguing the surface is unimportant. The boundary that makes this an exception and not a loophole: a shape a caller SENDS, or one that describes what a caller may send, is governed; a shape rendered for a third-party renderer is not. A projection that starts accepting writes stops being exempt.


ONE SURFACE IS KNOWN UNCONVERGED, and it is recorded here as an open item rather than an exception, because nothing about it earns exemption. The schema exemplar's authoring dialect (`ExemplarRelation` in `memstead-schema`, the built-in type YAMLs, and the authoring guide) still spells an edge `to:` / `type:`. This rule's own Scope claims prose and generated docs are governed "because a name that survives only in an example is still a name an agent will copy", and the exemplar is precisely an example an agent copies. The served payload was converged so the copy is accepted; the authoring file was not. What blocks it is mechanical, not principled: the current shipped built-in must validate as authoring content, and its bytes are sealed, so converging the dialect needs a version bump. The engine already has the strict-authoring / sealed-tolerant translate pattern this would use (see `propagating_relationships` in the schema loader). Tracked in `dev/backlog.md`. A reader consulting this principle should know the tree is not yet fully under it.

## Consequences

A rename is a wire break and lands as one: changelog entry naming the old key, propagation through every consumer in the same session, and no compatibility shim. The wire-shape tests that pinned the old asymmetry are updated as part of the change, because what they pinned was the defect, and a test is added that asserts the retired name refuses, so a later `serde(alias = ...)` added in kindness fails loudly.
