---
type: decision
created_date: 2026-09-11T00:04:49Z
last_modified: 2026-09-11T00:04:49Z
status: accepted
decided_on: 2026-09-11
deciders: architecture-seams bundle plan 04 (agent, under the standing engine-change rule)
scope: subsystem
tags: storage, backend, kernel, migration
---

# One write trait on the storage backend and one backend error

## Decision
We will keep exactly one write-side abstraction on the storage layer: MemBackend, with BackendError as its one error type. The earlier MemWriter trait (four write methods) and MemWriterError were folded into them on 2026-09-11; BackendError carries Path and HashMismatch (the commit-tip CAS conflict) directly instead of wrapping a second enum, the two implementors are named for what they are, FilesystemBackend and GitTreeBackend, and the engine maps a backend HashMismatch onto its own HashMismatch envelope in one From impl so the wire carries HASH_MISMATCH whichever level detected the conflict ([[engine--storage-backend-trait-surface]], [[engine--filesystem-backend]], [[engine--git-tree-backend]]).

## Context
The backend module had documented the collapse as in progress since the workspace-store rebuild: MemBackend was the full trait, MemWriter its write-side subset with its own error type, and BackendError wrapped MemWriterError transparently. Two names per concept and two error types for one write path. The 2026-09-10 architecture assessment and both of its counter-checks kept the collapse as the one recommendation whose end state the code itself already specified. The claim walk found one more thing: the writer trait's doc comment described a mapping of the commit-tip CAS conflict onto HASH_MISMATCH that the code never performed (a backend CAS conflict surfaced as MEM_ERROR); realising the documented contract was part of the fold.

## Consequences
- Library consumers rename (registry, ui-api and the wasm build did in the same session); the wire is unchanged except that a backend CAS conflict now reaches agents as HASH_MISMATCH with the live tip, as documented.
- A test in the git-branch crate pins the mapping at the engine boundary; no test drives the CAS conflict through the MCP server itself.
- The one-name-per-concept principle holds on the storage layer again; the engine mem's entities for the two backends were renamed with the types.
- Cost accepted: the HashMismatch envelope for a backend conflict carries an empty entity id, since the backend does not know which entity's commit lost.

## Relationships
- **REFERENCES**: [[engine:storage-backend-trait-surface]]
- **REFERENCES**: [[engine:filesystem-backend]]
- **REFERENCES**: [[engine:git-tree-backend]]

## Options

- Keep MemWriter as a thin alias over MemBackend: rejected, an alias is two names per concept.
- Keep MemWriterError as a nested variant of BackendError: rejected, the variants belong to the one error type and the transparent wrapper hid the CAS conflict from the engine mapping.
- Leave the documented CAS mapping unrealised: rejected, a documented contract the code does not perform is the defect class the assessment was about.
