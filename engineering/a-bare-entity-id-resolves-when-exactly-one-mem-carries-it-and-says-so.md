---
type: decision
created_date: 2026-09-03T12:46:41Z
last_modified: 2026-09-03T12:46:41Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C5 executing session
scope: subsystem
tags: cli, mcp, entity-id
---

# A bare entity id resolves when exactly one mem carries it, and says so

## Decision
We will resolve a bare entity id in one place, the engine's id resolver that every id-taking mutation verb calls before it reads the id's mem. A full id returns unchanged and silent; a malformed bare string is an invalid id; a well-formed slug is looked up across every mounted mem after the deferred ones load. Exactly one carrier resolves it and the verb announces `SHORT_ID_RESOLVED` on its outcome, on the human surface as well as under `--json`; zero or several carriers refuse `ENTITY_ID_MISSING_MEM` naming every full id that carries the slug.

## Context
Before this rule a bare slug reached the verbs as an id whose mem was the empty string, and the caller was told a mem called "" did not exist, with a recovery hint pointing nowhere. Resolving the unique case is safe precisely because the ambiguity that would make it unsafe is exactly the case that refuses.

## Consequences
- Every id-taking verb behaves identically, because the rule lives in the resolver rather than in the commands.
- The tradeoff accepted: a bare slug that resolves today can refuse tomorrow when a second mem gains the slug. The announcement says so, and a caller needing a fixed target writes the full id.
- A bare-slug mutation forces the deferred mems to load, so the first such call on a lazily mounted workspace pays a load it would otherwise defer.
- Announcing through a warning obliges every response renderer to render warnings; two markdown branches did not, and hid the announcement until they were fixed.

## Options

- Resolve when unique, refuse when ambiguous, announce either way: CHOSEN.
- Refuse every bare slug: rejected, because in the unique case the refusal tells the user to type the id the engine just computed.
- Resolve an ambiguous slug by mount order: rejected as the silent wrong-target write, since mount order is workspace configuration and not caller intent.
