---
type: decision
created_date: 2026-11-11T11:24:21Z
last_modified: 2026-11-11T11:24:21Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: errors, mcp, cli
---

# A refusal about engine state is one engine value that every surface renders

## Decision
We will raise every refusal about workspace, mem or entity state as one typed engine value, whose code, message and details the MCP server and the CLI render unchanged. A surface builds a code of its own only for its own input syntax or for an operation the surface itself performs (network, registry, login, signing keys, the server process, a command's own file IO); those codes are placed by the closed lists in xtask's errors module and a placement test reads every site that builds one.

## Context
The MCP mapper and the CLI built different payloads for the same error code, and the CLI built state codes such as ENTITY_NOT_FOUND and UNKNOWN_MEM by hand with its own text and details. An agent that learned a recovery from one surface could not apply it on the other.

## Consequences
- A parity test renders one sample of every engine error variant, and of each inner error type the engine wraps, through both surfaces; a variant without a sample fails the build or the test.
- Codes that duplicated an engine refusal were retired; each retirement is named in the changelog.
- Known limits of the gates: the placement test counts INVALID_INPUT sites per file, so a syntax site swapped for a state site inside one file passes, and the parity test compares the first line of the engine's message.
- A new refusal about state needs an engine variant and a sample before any surface can raise it.

## Relationships
- **GOVERNS**: [[engine:typed-error-and-warning-envelope]]
- **GOVERNS**: [[engine:mcp-tool-surface]]
- **GOVERNS**: [[engine:cli-command-surface]]

## Options

- Keep one mapper per surface and compare them in a test: rejected, because two mappers drift and the test only reports the drift after it shipped.
- Move every code into the engine, network and login included: rejected, because those are operations of the surface, not states of the engine.
- One engine value for every state refusal, with a closed list of surface-owned codes: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 04, graded by independent graders under the bundle's gate. Six graders recommended reading the rule as 'input syntax or the surface's own operation', which is the wording used here.
