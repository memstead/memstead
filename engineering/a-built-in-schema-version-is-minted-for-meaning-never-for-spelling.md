---
type: decision
created_date: 2026-08-28T03:48:31Z
last_modified: 2026-08-28T03:48:31Z
status: accepted
decided_on: 2026-08-28
deciders: operator
scope: system
tags: schema, versioning, naming, agent-surface
---

# A built-in schema version is minted for meaning, never for spelling

## Decision
We chose not to mint new built-in schema versions to rename a key in an exemplar's authoring YAML, and to treat the SERVED payload as the contract a caller is held to. The convergence of one relation spelling across every served input and output is complete without touching the six built-in families whose YAML exemplars still carry the retired key; the authoring dialect converges when the next built-in bump has a substantive reason of its own. This narrows the earlier posture in [[engineering--schema-vocabulary-says-only-true-things-via-leaf-declaration-and-the-self-loop-rename]], where every built-in carrying a retired key shipped a new version: that rename changed what the key MEANT, and this one changes only how it is spelled in a source format no caller reads.

## Context
A naming convergence had unified one relation spelling across the MCP, CLI and HTTP surfaces, with the retired spelling refused rather than aliased. The built-in schema packages' exemplar YAML still spelled it the old way. Two independent graders read the governing criterion oppositely: one held that an exemplar is a served shape and the YAML must converge, the other that the criterion quantifies over what a caller sends and receives. The disagreement blocked the work for two rounds, which is why the ruling is recorded rather than left to be re-derived.

## Consequences
The convergence closes without a migration. Six built-in families keep their current versions, so what `default` resolves to for new workspaces does not move and 42 type files are not duplicated. The cost is real and named rather than denied: whoever writes a schema by hand still reads the retired spelling in the authoring guide, and that stays filed as an open item rather than an exception, because nothing about it earns exemption. A second consequence is a rule for the next reader: a version bump needs a behavioural reason, and the sealed-translate path exists to carry the spelling change along when one arrives.

## Relationships
- **MOTIVATED_BY**: [[one-name-per-concept-across-every-surface-and-a-retired-name-refuses]]
- **REFERENCES**: [[schema-vocabulary-says-only-true-things-via-leaf-declaration-and-the-self-loop-rename]]

## Options

- Mint six new built-in versions now, converging the YAML with the served payload: rejected. It would be this catalogue's first purely cosmetic bump, permanently duplicating 42 type files across six families and moving what `default` resolves to for new workspaces, with no behavioural gain. The precedent cuts against it: the last bump was minted for metadata fields becoming opt-in required, a real semantic change.
- Declare the authoring YAML a permanent exception to the one-name rule: rejected. The cost to a hand-authoring reader is real, and calling it an exception would retire a live obligation by fiat. It is deferred with a named trigger instead.
- Rule that the served payload satisfies the criterion, and defer the YAML: chosen. The primary consumer is the agent, and the agent reads the served schema payload before writing; copying what it reads produces a legal edge. The YAML is a different audience and a different serialization, with a legitimate translation point between them.

## Notes

Recorded 2026-08-28 at the close of the consistency sweep's mutation-shape work. The deferred half rides the next built-in version bump that has a substantive reason; the working sealed-translate mechanism is already in place to carry it.
