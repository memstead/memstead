---
type: decision
created_date: 2026-09-11T07:24:32Z
last_modified: 2026-09-11T07:48:53Z
status: accepted
decided_on: 2026-09-11
deciders: one-provenance bundle plan 01 (agent, under the standing engine-change rule)
scope: subsystem
tags: provenance, commit-context, git-branch, kernel
---

# Every commit the engine writes carries the mutation's context, built in one place

## Decision
We will build every commit context the engine writes in one place and hand it to whatever writes the commit: `CommitContext::new` is the one constructor, the engine reaches it through `commit_context` (a mutation's tool, actor, client and note with the session's declared role and identity) or `session_commit_context` (the transport's actor and client, which the CLI sets at boot and the MCP server sets when its client initialises), and every backend writer that produces a commit takes the context it is handed and never fabricates one. The config writes on the schema-and-config ref, the binding-store edits, the force-overwrite prune and the mem rename therefore carry the same Tool, Actor, Client, Role and Identity trailers and the note as every entity mutation's commit ([[engine--create-mutation]], [[engine--git-tree-backend]]), and the Tool trailer names the operation that caused the write.

## Context
The prune commit of a mem deletion learned its trailers on 2026-09-11 (seams follow-up bundle); the same day a grader read a create's config-write commit carrying `Tool: memstead_mem_config_write` and `Actor: agent` only. The cause was the shape of the backend trait: `write_mem_config` took bytes, `write_mem_config_with_note` bytes and a note, the pipeline-edit writer a note, and each git-tree implementation built a context of its own with a default role and no identity; the storage dispatchers for the force-overwrite prune and the mem rename did the same, and the CLI's own context helper defaulted the role and identity the CLI had been given as flags. Two anchor writers and the mem sweep hard-coded the agent actor for commits a CLI verb causes. The independent grade of the change found one more path of the same class: the CLI's `projection init`, `projection enable` and `projection edit` wrote the binding record straight to the store with no commit at all, while the same edit through MCP committed with a note, so the CLI's binding edits left no trace in the mem-repo; the three now write through the engine's record writer and take a note. The engineering principle that a guard on one write path exists on all of them with one implementation ([[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]) names the class: per-path copies of the provenance context rotted independently, each missing a different trailer.

## Consequences
- A reader of the mem-repo's history sees who did what on every commit, config writes and binding edits included; the notes surface reads the same trailers everywhere.
- The Tool trailer of a config-write commit names the operation (set_mem_version, set_mem_sync_state, stamp_mutation_versions, memstead_mem_create) where it named the backend's generic writer; the Actor of the anchor writers and the mem sweep follows the transport (cli under the CLI).
- A backend that produces no commit ignores the context, as the folder and in-memory backends do for the delete; a writer that needs a context before any session exists (the boot-time readMems migration, the serve service's own seed) builds one through the constructor with an unspecified role and no identity, recorded as absence.
- Adding a commit-producing operation means passing a context, not writing one: the one literal in the tree is the constructor, and a test that reads every commit a session wrote back from the mem-repo pins the trailers.

## Relationships
- **REFERENCES**: [[engine:create-mutation]]
- **REFERENCES**: [[engine:git-tree-backend]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

- Add role and identity to every backend writer's signature one by one: rejected, four signatures growing the same three fields is the copy the change removes.
- Keep the backend's own context for config writes as engine bookkeeping: rejected, a sync-state stamp or a version bump under a declared identity is an act that identity performed, and the commit is its only durable record.
- Store the session's role and identity on the backend at mount time: rejected, the MCP surface changes them per call and a backend must not remember a caller.
- Leave the Tool trailer of config writes as the generic writer's name: rejected, the trailer exists to say what caused the commit.
