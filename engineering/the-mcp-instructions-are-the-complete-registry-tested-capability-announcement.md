---
type: decision
created_date: 2026-08-08T09:59:17Z
last_modified: 2026-08-24T12:18:20Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust bundle plan 05)
scope: subsystem
tags: mcp, instructions, capability-blindness, agent-surface, versioning
---

# The MCP instructions are the complete registry-tested capability announcement

## Decision
An agent's session start answers "what can this engine do, on which surface, at which version" from the MCP server instructions alone. Both server flavours' instructions carry: the engine version (baked at compile time via `concat!` + `env!`, matching `serverInfo.version` — the hardcoded `"0.1.0"` is structurally gone because `get_info` is hand-written from the crate version); the complete grouped tool roster (name-plus-clause, never paragraphs); and a CLI-companion note naming the verb families that deliberately do not live on this surface (batch mutation, export/install, distribution, bootstrap/repair, workspace policy) and when to reach for them. "Not on MCP" is not "CLI only": workspace policy is an operator surface served by both the CLI and the operator-authenticated web API, and the note is worded so it does not assert exclusivity it cannot back. `memstead_overview` frontmatter carries `_engine_version`. The text is registry-tested: a bidirectional test derives the roster from the live tool registration and fails when the instructions lag OR lead the registry; the version is pinned to the crate by test; a byte-budget tripwire bounds growth (the error-code list, not the roster, is the sanctioned cut when room is needed). The instructions live as one named `pub const` per flavour — the tests read the same string the handler serves, so the historical duplicated-copy drift risk is gone.

## Context
The costliest failure class in the two field projects, measured in thrown-away work, was capability blindness: an agent hand-rolled 175 single-entity calls while `batch-create` existed one surface over, re-implemented `list --filter`, and reported "a mem cannot carry its subject" the day after `mem set-subject` shipped. The root cause was missing announcement, not missing capability: the instructions enumerated ~60 error codes by name yet listed only 13 of 24 tools, named no version (worse — served a wrong hardcoded one), and never mentioned that a CLI companion surface exists. Nothing tied the text to the registry, so every new tool silently widened the gap.

## Consequences
A returning agent that sees a changed `_engine_version` knows to re-read the roster — that plus the complete roster is the "what changed" answer with no new tool and no capability-changelog surface (deferred; revisit only if capability blindness recurs WITH this landed). MCP `tools/list` stays as the protocol channel but is insufficient alone — it cannot carry the cross-surface note, the version, or workflow grouping, and instruction text is what models attend to at session start. Every new tool now forces an instruction-roster edit at registration time (the bidirectional test is the forcing function). The rmcp `tool_handler` macro only accepts string literals, so both flavours hand-write `get_info` — a future macro upgrade could fold that back.

## Relationships
- **INFORMED_BY**: [[refusals-pre-announce-across-gates-additive-in-the-later-gates-own-shape-never-merged-envelopes]]

## Options

Relying on `tools/list` alone was rejected (cannot carry the cross-surface note — the actual cause of the wasted work). A served capability-changelog ("what's new since version X") was deferred as surface cost without field evidence of need. Moving the error-code list out of the instructions entirely was not taken — a separate judgment; this plan only forbids the roster from being what gets crowded out.

## Notes


