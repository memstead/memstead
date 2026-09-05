---
type: decision
created_date: 2026-09-05T15:08:54Z
last_modified: 2026-09-05T15:08:54Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: binding, projection, schema, vocabulary
---

# A binding's intent is validated by the engine against the destination vocabulary

## Decision
An all-caps token of three or more characters in a binding's intent is read as a relationship of the destination mem's schema, because that is how an agent reads it: an edge it may write. The engine checks the intent against the vocabulary the destination mem pins (main relationships and cross-mem blocks alike) whenever the binding is loaded for a brief or a verify, and reports each undeclared token as a typed finding, BINDING_INTENT_UNKNOWN_RELATIONSHIP, naming the token, the schema pin and the vocabulary: on the build, verify and sync briefs next to the intent, and on the verify report as report.intent_findings. Load never refuses: a record written before the rule keeps loading and surfaces the defect. Writes refuse: projection init with an intent, and a projection edit patch that sets one, refuse with the same code and write nothing; a patch that preserves or clears the intent is not a new intent and passes. Both write doors boot a scoped engine (the destination mem only) to read that schema, so a workspace store the engine cannot parse refuses the write typed. A file name (CLAUDE.md, the extension says so) and a short list of protocol and format acronyms (HTTP, JSON, MCP) read as prose; a vocabulary relationship always wins over that exemption. A destination mem that is not mounted or carries no schema has no vocabulary to read, so the rule does not apply there and the brief's absent-destination note carries the case.

## Context
The dogfood engine binding once named PROVIDED_BY against the software schema and a memstead-pro-* crate family that did not exist, and a sync agent read both as facts about the code (model-truth storey 2, findings 1 and 2). The workspace script scripts/check-binding-intent.mjs caught the class from outside the engine; the 2026-09-04 census (posture C) moved it inside so the check runs wherever a binding is used, not only on one CI job over one binding. The crate-name half of the script is a workspace convention (memstead-<name> must be a crate directory) and was dropped rather than generalised: a generic 'named path exists' rule would guess or need configuration nobody else has. The binding record itself is the one-record-per-obligation shape of [[engineering--collapse-the-pipeline-into-one-versioned-binding-per-source-to-mem-obligation]].

## Consequences
Every binding in every workspace gets the check for free, against its own schema, with no script and no manifest. The refusal code sits in the generated error index on both the engine and the CLI surface. Rejected shape: a schema constraint kind on the binding record, because the record is engine-owned state, not a schema-typed entity; the validation belongs to the projection loader and the edit layer (pipeline_edit's _against variants carry the destination schema, the engine's add/update projection methods pass their own). The prose-acronym exemption is a fixed list, the one non-generic element, kept small and documented; a schema that declares one of those names as a relationship gets it recognised as one.

## Relationships
- **REFERENCES**: [[collapse-the-pipeline-into-one-versioned-binding-per-source-to-mem-obligation]]
