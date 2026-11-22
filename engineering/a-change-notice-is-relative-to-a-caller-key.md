---
type: decision
created_date: 2026-11-22T13:15:47Z
last_modified: 2026-11-22T18:00:38Z
status: accepted
decided_on: 2026-11-21
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: reload,change-notice,mcp,library
---

# A change notice is relative to a caller key

## Decision
We chose to make a change notice relative to a caller key that the surface opening the operation hands in. The key is not an identity. The default key is the single caller of a surface, known since the engine's first reload and therefore never a first contact: the first instance of a shared MCP server is told from engine start on. A caller seen for the first time starts from what storage holds, so nothing that landed before it came is ever told to it. A notice covers everything storage holds for its mem: the engine catches up before it builds one. The `MEM_RELOADED` warning is the text form of that same notice, built from what the caller is told and from nothing else. Over MCP the notices that a tool call's operations record belong to that call, and the dispatch adds what the tool left out; a call that is not run leaves the notice for the next call. The engine remembers 1024 callers and forgets the least recently used.

## Context
One engine behind a server with several callers told a change once, to whichever caller operated next. The writer was told about its own write, another caller was told nothing, and a caller that came late was told about history it had no use for. The notice was a property of the engine, where it has to be a property of who is asking.

## Consequences
- Several server instances over one engine are each told what the others wrote, exactly once, and never about their own writes.
- For the single caller `memstead_check`, `memstead_mem_delete` and refused calls, which used to drop a notice the engine had produced, now carry it: the one deliberate deviation from 0.26.1.
- An operation scope dropped without `finish` hands out nothing; the MCP tool scope finishes on drop.
- A caller forgotten after 1024 others starts again from storage and is told nothing of what it missed.

## Relationships
- **DERIVED_FROM**: [[reload-notices-ride-an-operation-scope-and-nothing-on-the-engine-hands-them-out-on-demand]]

## Options

- Key the notice by declared identity: rejected, an identity is who writes, and two sessions under one identity are two readers.
- Tell every caller everything since engine start: rejected, a late caller would be told history.
- A caller key handed in by the surface: chosen.
