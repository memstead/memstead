---
type: decision
created_date: 2026-11-23T11:12:59Z
last_modified: 2026-11-23T11:12:59Z
status: accepted
decided_on: 2026-11-23
deciders: operator (backlog bundle, go of 2026-11-23), implementing agent
scope: system
tags: quarantine,errors,cli,mcp,mounts
---

# One function refuses a name that resolved to no mem in service

## Decision
We chose to give the engine one function that refuses a name which resolved to no mem in service, and to make it the only way to build that refusal inside a booted engine. It answers `MEM_QUARANTINED` with the reason when the roster carries the name and the mem is out of service, `MEM_UNMOUNTED` when the mem left the roster a moment ago, and `UNKNOWN_MEM` otherwise. The plain constructor of `UNKNOWN_MEM` is not public and has that function as its one caller. The refusal for an entity that is in no view asks it first, so an id whose mem is out of service is answered at the mem grain: the engine has not read that mem and does not claim that the entity is missing. A read that takes a mem or an entity as its scope refuses the same way, where it would otherwise print an empty reading.

## Context
A quarantined mem is on the roster and in no view. Entity reads, writes and the MCP tools said so. About nineteen call sites in three crates built the unknown-mem refusal by a plain constructor that knew nothing of the quarantine roster, the entity read carried its own copy of the check, and four readings (`due --mem`, `gates --mem`, `anchors <id>`, a search related to an entity) printed nothing. On those verbs a caller could not tell a mem that is out of service from a mistyped name.

## Consequences
- A call site cannot pick a form that forgets the quarantine roster, outside the engine module tree.
- The verbs that read the whole workspace keep their own refusal, `MEM_VIEW_STALE` with cause `out_of_service`, which names the mem.
- `due --mem` and `gates --mem` refuse a name nobody knows, where they printed an empty reading.
- The repair path that runs below boot has no engine and no quarantine roster and builds its own refusal for a name the workspace description does not carry.
- A reading of the workspace's check ledger for a mem that is not mounted still answers, with a notice.

## Relationships
- **IMPLEMENTS**: [[degrade-never-disappear]]

## Options

- Fix the two verbs the backlog named: rejected, the same plain form stood at every other call site.
- Keep two functions and scan the source for the plain one: rejected, a gate over a cause that can be removed.
- Answer `ENTITY_NOT_FOUND` with a hint for an entity of a quarantined mem: rejected, the engine has not read the mem and does not know that the entity is missing.
- One function, the plain constructor private to it: chosen.
